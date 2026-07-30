use crate::bind::segment::{ContourIndex, IdSegment, IdSegments};
use crate::bind::solver::{ShapeBinder, SortByAngle};
use crate::core::edge_data::OverlayEdgeData;
use crate::core::extract::{BooleanExtractionBuffer, GraphUtil, Visit, VisitState};
use crate::core::graph::OverlayGraph;
use crate::core::link::{OverlayLink, OverlayLinkFilter};
use crate::core::overlay::ContourDirection;
use crate::core::overlay_rule::OverlayRule;
use crate::geom::v_segment::{BottomSegment, VSegment};
use crate::segm::segment::SegmentFill;
use crate::vector::edge::{DataVectorEdge, DataVectorPath, DataVectorShape};
use crate::vector::simplify::VectorSimplify;
use alloc::vec;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_float::int::number::uint::UIntNumber;
use i_float::int::number::wide_int::WideIntNumber;
use i_float::int::point::IntPoint;
use i_key_sort::sort::key::SortKey;
use i_tree::Expiration;

impl<I, D> OverlayGraph<'_, I, D>
where
    I: IntNumber + Expiration + SortKey,
    D: OverlayEdgeData,
{
    pub fn extract_separate_vectors(&self) -> Vec<DataVectorEdge<I>> {
        self.links
            .iter()
            .map(|link| DataVectorEdge::new(link.fill, link.a.point, link.b.point, ()))
            .collect()
    }

    pub fn extract_vector_shapes(
        &self,
        overlay_rule: OverlayRule,
        buffer: &mut BooleanExtractionBuffer<I>,
    ) -> Vec<DataVectorShape<I, D>> {
        let mut store = D::Store::default();
        self.extract_vector_shapes_with_store(overlay_rule, buffer, &mut store)
    }

    pub fn extract_vector_shapes_with_store(
        &self,
        overlay_rule: OverlayRule,
        buffer: &mut BooleanExtractionBuffer<I>,
        store: &mut D::Store,
    ) -> Vec<DataVectorShape<I, D>> {
        let clockwise = self.options.output_direction == ContourDirection::Clockwise;
        self.links
            .filter_by_overlay_into(overlay_rule, &mut buffer.visited);

        let mut holes = Vec::new();
        let mut shapes = Vec::new();
        let mut anchors = Vec::new();

        let mut link_index = 0;
        let mut anchors_already_sorted = true;
        while link_index < buffer.visited.len() {
            if buffer.visited.is_visited(link_index) {
                link_index += 1;
                continue;
            }

            let left_top_link = unsafe {
                // SAFETY: `link_index` walks 0..buffer.visited.len(), and buffer.visited.len() <= self.links.len().
                GraphUtil::find_left_top_link(self.links, self.nodes, link_index, &buffer.visited)
            };

            let link = unsafe {
                // SAFETY: `left_top_link` came from `find_left_top_link`, which only
                // ever returns indices in 0..self.links.len().
                self.links.get_unchecked(left_top_link)
            };

            let is_hole = overlay_rule.is_fill_top(link.fill);
            let visited_state = [VisitState::HullVisited, VisitState::HoleVisited][is_hole as usize];

            let direction = is_hole == clockwise;
            let start_data = StartVectorPathData::new(direction, link, left_top_link);

            let mut contour =
                self.find_vector_contour(start_data, direction, visited_state, &mut buffer.visited, store);
            let (is_valid, is_modified) = contour.validate(
                self.options.min_output_area,
                self.options.preserve_output_collinear,
            );

            if !is_valid {
                link_index += 1;
                continue;
            }

            if is_hole {
                let left_bottom = if clockwise { contour[1].a } else { contour[0].a };
                let mut v_segment = most_left_bottom_from(&contour, left_bottom);

                if is_modified {
                    let most_left = most_left_bottom(&contour);
                    if most_left != v_segment {
                        v_segment = most_left;
                        anchors_already_sorted = false;
                    }
                };

                debug_assert!(v_segment == most_left_bottom(&contour));
                let id_data = ContourIndex::new_hole(holes.len());
                anchors.push(IdSegment::with_segment(id_data, v_segment));
                holes.push(contour);
            } else {
                shapes.push(vec![contour]);
            }
        }

        if !anchors_already_sorted {
            anchors.sort_by_key(|s0| s0.v_segment.a);
        }

        shapes.join_sorted_holes(holes, anchors, clockwise);

        shapes
    }

    fn find_vector_contour(
        &self,
        start_data: StartVectorPathData<I, D>,
        clockwise: bool,
        visited_state: VisitState,
        visited: &mut [VisitState],
        store: &mut D::Store,
    ) -> DataVectorPath<I, D> {
        let mut link_id = start_data.link_id;
        let mut node_id = start_data.node_id;
        let last_node_id = start_data.last_node_id;

        visited.visit_edge(link_id, visited_state);

        let mut contour = DataVectorPath::new();
        contour.push(DataVectorEdge::new_with_store(
            start_data.fill,
            start_data.a,
            start_data.b,
            start_data.data,
            store,
        ));

        let last_link_id =
            GraphUtil::next_link(self.links, self.nodes, link_id, last_node_id, !clockwise, visited);

        // Find a closed tour
        while link_id != last_link_id {
            link_id = GraphUtil::next_link(self.links, self.nodes, link_id, node_id, clockwise, visited);

            let link = unsafe {
                // SAFETY: `link_id` is always a valid link index obtained from the
                // traversal helpers, so this stays in-bounds.
                self.links.get_unchecked(link_id)
            };
            node_id = contour.push_node_and_get_other(link, node_id, store);

            visited.visit_edge(link_id, visited_state);
        }

        contour
    }
}

impl<I: IntNumber, D: OverlayEdgeData> OverlayGraph<'_, I, D> {
    pub fn extract_vectors(&self) -> Vec<DataVectorEdge<I, D>> {
        let mut store = D::Store::default();
        self.extract_vectors_with_store(&mut store)
    }

    pub fn extract_vectors_with_store(&self, store: &mut D::Store) -> Vec<DataVectorEdge<I, D>> {
        self.links
            .iter()
            .map(|link| {
                DataVectorEdge::new_with_store(link.fill, link.a.point, link.b.point, link.data, store)
            })
            .collect()
    }
}

struct StartVectorPathData<I: IntNumber, D> {
    a: IntPoint<I>,
    b: IntPoint<I>,
    node_id: usize,
    link_id: usize,
    last_node_id: usize,
    fill: SegmentFill,
    data: D,
}

impl<I: IntNumber, D: OverlayEdgeData> StartVectorPathData<I, D> {
    #[inline(always)]
    fn new(direction: bool, link: &OverlayLink<I, D>, link_id: usize) -> Self {
        if direction {
            Self {
                a: link.b.point,
                b: link.a.point,
                node_id: link.a.id,
                link_id,
                last_node_id: link.b.id,
                fill: link.fill,
                data: link.data,
            }
        } else {
            Self {
                a: link.a.point,
                b: link.b.point,
                node_id: link.b.id,
                link_id,
                last_node_id: link.a.id,
                fill: link.fill,
                data: link.data,
            }
        }
    }
}

trait JoinHoles<I, D>
where
    I: IntNumber + Expiration + SortKey,
    D: OverlayEdgeData,
{
    fn join_sorted_holes(
        &mut self,
        holes: Vec<DataVectorPath<I, D>>,
        anchors: Vec<IdSegment<I>>,
        clockwise: bool,
    );
    fn scan_join(
        &mut self,
        holes: Vec<DataVectorPath<I, D>>,
        hole_segments: Vec<IdSegment<I>>,
        clockwise: bool,
    );
}

impl<I, D> JoinHoles<I, D> for Vec<DataVectorShape<I, D>>
where
    I: IntNumber + Expiration + SortKey,
    D: OverlayEdgeData,
{
    fn join_sorted_holes(
        &mut self,
        holes: Vec<DataVectorPath<I, D>>,
        anchors: Vec<IdSegment<I>>,
        clockwise: bool,
    ) {
        if self.is_empty() || holes.is_empty() {
            return;
        }

        if self.len() == 1 {
            let mut hole_paths = holes;
            self[0].append(&mut hole_paths);
            return;
        }
        debug_assert!(is_sorted(&anchors));

        let mut anchors = anchors;
        anchors.add_sort_by_angle();
        self.scan_join(holes, anchors, clockwise);
    }

    fn scan_join(
        &mut self,
        holes: Vec<DataVectorPath<I, D>>,
        hole_segments: Vec<IdSegment<I>>,
        clockwise: bool,
    ) {
        let x_min = hole_segments[0].v_segment.a.x;
        let x_max = hole_segments[hole_segments.len() - 1].v_segment.a.x;

        let capacity = self.iter().fold(0, |s, it| s + it[0].len()) / 2;
        let mut segments = Vec::with_capacity(capacity);
        for (i, shape) in self.iter().enumerate() {
            shape[0].append_id_segments(&mut segments, ContourIndex::new_shape(i), x_min, x_max, clockwise);
        }

        for (i, hole) in holes.iter().enumerate() {
            hole.append_id_segments(&mut segments, ContourIndex::new_hole(i), x_min, x_max, clockwise);
        }

        segments.sort_by_a_then_by_angle();

        let solution = ShapeBinder::bind(self.len(), hole_segments, segments);

        for (shape_index, &capacity) in solution.children_count_for_parent.iter().enumerate() {
            self[shape_index].reserve_exact(capacity);
        }

        for (hole_index, hole) in holes.into_iter().enumerate() {
            let shape_index = solution.parent_for_child[hole_index];
            self[shape_index].push(hole);
        }
    }
}

#[inline]
fn most_left_bottom<I: IntNumber, D>(path: &DataVectorPath<I, D>) -> VSegment<I> {
    let mut a = path[0].a;
    for e in path.iter().skip(1) {
        if e.a < a {
            a = e.a;
        }
    }

    most_left_bottom_from(path, a)
}

#[inline]
fn most_left_bottom_from<I: IntNumber, D>(path: &DataVectorPath<I, D>, a: IntPoint<I>) -> VSegment<I> {
    let n = path.len();
    let mut result: Option<VSegment<I>> = None;

    for (i, edge) in path.iter().enumerate() {
        if edge.a != a {
            continue;
        }

        // Self-touching contours can visit the left-bottom point several times.
        // Check every incident edge at that point and keep the lowest anchor edge.
        let b0 = edge.b;
        let b1 = path[(i + n - 1) % n].a;
        result.update_if_under(VSegment { a, b: b0 });
        result.update_if_under(VSegment { a, b: b1 });
    }

    result.unwrap_or(VSegment { a, b: a })
}

#[inline]
fn is_sorted<I: IntNumber>(segments: &[IdSegment<I>]) -> bool {
    segments
        .windows(2)
        .all(|slice| slice[0].v_segment.a <= slice[1].v_segment.a)
}

trait DataGraphContour<I: IntNumber, D: OverlayEdgeData> {
    fn validate(&mut self, min_output_area: I::WideUInt, preserve_output_collinear: bool) -> (bool, bool);
    fn push_node_and_get_other(
        &mut self,
        link: &OverlayLink<I, D>,
        node_id: usize,
        store: &mut D::Store,
    ) -> usize;
}

impl<I: IntNumber, D: OverlayEdgeData> DataGraphContour<I, D> for DataVectorPath<I, D> {
    #[inline]
    fn validate(&mut self, min_output_area: I::WideUInt, preserve_output_collinear: bool) -> (bool, bool) {
        let is_modified = if !preserve_output_collinear {
            self.simplify_contour()
        } else {
            false
        };

        if self.len() < 3 {
            return (false, is_modified);
        }

        if min_output_area == I::WideUInt::ZERO {
            return (true, is_modified);
        }

        let double_area = self
            .iter()
            .fold(I::Wide::ZERO, |acc, edge| acc + edge.a.cross_product(edge.b));

        ((double_area.unsigned_abs() >> 1) >= min_output_area, is_modified)
    }

    #[inline]
    fn push_node_and_get_other(
        &mut self,
        link: &OverlayLink<I, D>,
        node_id: usize,
        store: &mut D::Store,
    ) -> usize {
        if link.a.id == node_id {
            self.push(DataVectorEdge::new_with_store(
                link.fill,
                link.a.point,
                link.b.point,
                link.data,
                store,
            ));
            link.b.id
        } else {
            self.push(DataVectorEdge::new_with_store(
                link.fill,
                link.b.point,
                link.a.point,
                link.data,
                store,
            ));
            link.a.id
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::core::fill_rule::FillRule;
    use crate::core::overlay::{ContourDirection, IntOverlayOptions, Overlay};
    use crate::core::overlay_rule::OverlayRule;
    use i_shape::int_shape;

    #[test]
    fn test_keep_output_points_0() {
        #[rustfmt::skip]
        let subj = int_shape![
            [[0, 0], [2, 0], [2, 2], [0, 2]],
            [[2, 0], [4, 0], [4, 2], [2, 2]],
        ];
        let mut buffer = Default::default();

        let mut overlay = Overlay::with_contours(&subj, &[]);
        overlay.options = IntOverlayOptions::keep_all_points();
        let shapes = overlay
            .build_graph_view(FillRule::NonZero)
            .unwrap()
            .extract_vector_shapes(OverlayRule::Subject, &mut buffer);

        debug_assert!(shapes[0][0].len() == 6);

        let mut overlay = Overlay::with_contours(&subj, &[]);
        overlay.options = IntOverlayOptions::default();
        let shapes = overlay
            .build_graph_view(FillRule::NonZero)
            .unwrap()
            .extract_vector_shapes(OverlayRule::Subject, &mut buffer);

        debug_assert!(shapes[0][0].len() == 4);
    }

    #[test]
    fn test_keep_output_points_1() {
        #[rustfmt::skip]
        let subj = int_shape![
            [[0, 0], [3, 0], [3, -3], [0, -3], [0, -1], [1, -1], [1, -3], [0, -3]],
        ];

        let mut buffer = Default::default();
        let mut overlay = Overlay::with_contours(&subj, &[]);
        overlay.options = IntOverlayOptions::default();
        let shapes = overlay
            .build_graph_view(FillRule::NonZero)
            .unwrap()
            .extract_vector_shapes(OverlayRule::Subject, &mut buffer);

        debug_assert!(shapes[0][0].len() == 4);
    }

    #[test]
    fn test_0() {
        #[rustfmt::skip]
        let subj = int_shape![
            [[0, 0], [4, 0], [4, 4], [0, 4]],
            [[1, 1], [1, 3], [3, 3], [3, 1]],
        ];

        let mut buffer = Default::default();
        let mut overlay = Overlay::with_contours(&subj, &[]);

        let shapes_0 = overlay
            .build_graph_view(FillRule::NonZero)
            .unwrap()
            .extract_vector_shapes(OverlayRule::Subject, &mut buffer);

        debug_assert!(shapes_0.len() == 1);

        overlay.options.output_direction = ContourDirection::Clockwise;

        let shapes_1 = overlay
            .build_graph_view(FillRule::NonZero)
            .unwrap()
            .extract_vector_shapes(OverlayRule::Subject, &mut buffer);

        debug_assert!(shapes_1.len() == 1);
    }

    #[test]
    fn test_1() {
        #[rustfmt::skip]
        let subj = int_shape![
            [[2, 3], [3, 3], [3, 4], [2, 4]],
            [[1, 3], [1, 4], [2, 4], [2, 3]]
        ];
        let mut buffer = Default::default();
        let mut overlay = Overlay::with_contours(&subj, &[]);
        overlay.options = IntOverlayOptions::default();
        let shapes = overlay
            .build_graph_view(FillRule::NonZero)
            .unwrap()
            .extract_vector_shapes(OverlayRule::Subject, &mut buffer);

        debug_assert!(shapes.len() == 1);
        debug_assert!(shapes[0][0].len() == 4);
    }

    #[test]
    fn test_2() {
        #[rustfmt::skip]
        let subj = int_shape![
            [[0, 0], [3, 0], [3, -3], [2, -3], [2, 0], [-1, 0], [-1, 3], [-2, 3], [-2, 2], [0, 2], [0, 1], [-3, 1], [-3, 4], [0, 4]],
        ];
        let mut buffer = Default::default();
        let mut overlay = Overlay::with_contours(&subj, &[]);
        overlay.options = IntOverlayOptions::default();
        let shapes = overlay
            .build_graph_view(FillRule::NonZero)
            .unwrap()
            .extract_vector_shapes(OverlayRule::Subject, &mut buffer);

        debug_assert!(shapes.len() == 2);
    }

    #[test]
    fn test_3() {
        #[rustfmt::skip]
        let subj = int_shape![
            [[5, 2], [10, 2], [10, 3], [5, 3]],
            [[4, 0], [5, 0], [5, 4], [4, 4]],
            [[5, 7], [10, 7], [10, 8], [5, 8]],
            [[6, 6], [8, 6], [8, 10], [6, 10]],
            [[0, 3], [1, 3], [1, 7], [0, 7]],
            [[4, 4], [11, 4], [11, 6], [4, 6]],
            [[7, 1], [11, 1], [11, 5], [7, 5]],
        ];
        let mut buffer = Default::default();
        let mut overlay = Overlay::with_contours(&subj, &[]);
        overlay.options = IntOverlayOptions::default();
        let shapes = overlay
            .build_graph_view(FillRule::NonZero)
            .unwrap()
            .extract_vector_shapes(OverlayRule::Subject, &mut buffer);

        debug_assert!(shapes.len() == 2);
    }

    #[test]
    fn test_self_touching_contour_closes_by_edge() {
        #[rustfmt::skip]
        let subj = int_shape![
            [[-5, 0], [0, 0], [0, 5]],
            [[-3, 2], [-1, 2], [-1, 1]],
        ];

        let mut buffer = Default::default();
        let mut overlay = Overlay::with_contours(&subj, &[]);
        overlay.options = IntOverlayOptions {
            preserve_input_collinear: false,
            output_direction: ContourDirection::CounterClockwise,
            preserve_output_collinear: true,
            min_output_area: 0u64,
            ogc: false,
        };

        let shapes = overlay
            .build_graph_view(FillRule::NonZero)
            .unwrap()
            .extract_vector_shapes(OverlayRule::Subject, &mut buffer);

        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].len(), 1);
        assert_eq!(shapes[0][0].len(), 7);
    }
}
