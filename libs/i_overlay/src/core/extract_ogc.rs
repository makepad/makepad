use crate::bind::segment::{ContourIndex, IdSegment};
use crate::bind::solver::{JoinHoles, LeftBottomSegment};
use crate::core::extract::{
    BooleanExtractionBuffer, GraphContour, GraphUtil, StartPathData, Visit, VisitState,
};
use crate::core::graph::OverlayGraph;
use crate::core::overlay::ContourDirection;
use crate::core::overlay_rule::OverlayRule;
use alloc::vec;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_float::int::point::IntPoint;
use i_key_sort::sort::key::SortKey;
use i_shape::int::shape::{IntShape, IntShapes};
use i_shape::util::reserve::Reserve;
use i_tree::Expiration;

impl<I> OverlayGraph<'_, I>
where
    I: IntNumber + Expiration + SortKey,
{
    pub(crate) fn extract_ogc(
        &self,
        overlay_rule: OverlayRule,
        buffer: &mut BooleanExtractionBuffer<I>,
    ) -> IntShapes<I> {
        let is_main_dir_cw = self.options.output_direction == ContourDirection::Clockwise;

        let mut contour_visited = if let Some(mut visited) = buffer.contour_visited.take() {
            let target_len = buffer.visited.len();
            if visited.len() != target_len {
                visited.resize(target_len, VisitState::Skipped);
            }
            visited.fill(VisitState::Skipped);
            visited
        } else {
            vec![VisitState::Skipped; buffer.visited.len()]
        };

        let mut shapes = Vec::new();

        buffer.points.reserve_capacity(buffer.visited.len());
        let mut hole_count_hint = 0;

        let mut link_index = 0;
        while link_index < buffer.visited.len() {
            if buffer.visited.is_visited(link_index) {
                link_index += 1;
                continue;
            }

            let left_top_link = unsafe {
                // Safety: `link_index` walks 0..buffer.visited.len(), and buffer.visited.len() <= self.links.len().
                GraphUtil::find_left_top_link(self.links, self.nodes, link_index, &buffer.visited)
            };

            let link = unsafe {
                // Safety: `left_top_link` originates from `find_left_top_link`, which only returns
                // indices in 0..self.links.len(), so this lookup cannot go out of bounds.
                self.links.get_unchecked(left_top_link)
            };
            let is_hole = overlay_rule.is_fill_top(link.fill);
            let visited_state = [VisitState::HullVisited, VisitState::HoleVisited][is_hole as usize];

            let direction = is_hole == is_main_dir_cw;
            let traversal_direction = !is_main_dir_cw;

            let start_data = StartPathData::new(direction, link, left_top_link);

            if is_hole {
                // we will collect holes in a second pass
                self.skip_contour(
                    &start_data,
                    traversal_direction,
                    visited_state,
                    &mut buffer.visited,
                );
                hole_count_hint += 1;
                continue;
            }

            if let Some(shape) = self.collect_shape(
                &start_data,
                traversal_direction,
                &mut buffer.visited,
                &mut contour_visited,
                &mut buffer.points,
            ) {
                shapes.push(shape);
            } else {
                link_index += 1;
            };
        }

        if hole_count_hint > 0 {
            // Keep only hole edges; skip everything else for the second pass.
            for state in buffer.visited.iter_mut() {
                *state = match *state {
                    VisitState::HoleVisited => VisitState::Unvisited,
                    _ => VisitState::Skipped,
                };
            }

            let mut holes = Vec::with_capacity(hole_count_hint);
            let mut anchors = Vec::with_capacity(hole_count_hint);
            let mut anchors_already_sorted = true;
            link_index = 0;

            while link_index < buffer.visited.len() {
                if buffer.visited.is_visited(link_index) {
                    link_index += 1;
                    continue;
                }

                let left_top_link = unsafe {
                    // Safety: `link_index` walks 0..buffer.visited.len(), and buffer.visited.len() <= self.links.len().
                    GraphUtil::find_left_top_link(self.links, self.nodes, link_index, &buffer.visited)
                };

                let link = unsafe {
                    // Safety: `left_top_link` originates from `find_left_top_link`, which only returns
                    // indices in 0..self.links.len(), so this lookup cannot go out of bounds.
                    self.links.get_unchecked(left_top_link)
                };

                debug_assert!(overlay_rule.is_fill_top(link.fill));

                let start_data = StartPathData::new(is_main_dir_cw, link, left_top_link);

                self.find_contour(
                    &start_data,
                    is_main_dir_cw,
                    VisitState::HullVisited,
                    &mut buffer.visited,
                    &mut buffer.points,
                );

                let (is_valid, is_modified) = buffer.points.validate(
                    self.options.min_output_area,
                    self.options.preserve_output_collinear,
                );

                if !is_valid {
                    link_index += 1;
                    continue;
                }
                let contour = buffer.points.as_slice().to_vec();

                let left_bottom = if is_main_dir_cw { contour[1] } else { contour[0] };
                let mut v_segment = contour.left_bottom_segment_from(left_bottom);

                if is_modified {
                    let most_left = contour.left_bottom_segment();
                    if most_left != v_segment {
                        v_segment = most_left;
                        anchors_already_sorted = false;
                    }
                };

                debug_assert!(v_segment == contour.left_bottom_segment());
                let id_data = ContourIndex::new_hole(holes.len());
                anchors.push(IdSegment::with_segment(id_data, v_segment));
                holes.push(contour);
            }

            if !anchors_already_sorted {
                anchors.sort_unstable_by_key(|s0| s0.v_segment.a);
            }

            shapes.join_sorted_holes(holes, anchors, is_main_dir_cw);
        }

        buffer.contour_visited = Some(contour_visited);

        shapes
    }

    fn skip_contour(
        &self,
        start_data: &StartPathData<I>,
        clockwise: bool,
        visited_state: VisitState,
        visited: &mut [VisitState],
    ) {
        let mut link_id = start_data.link_id;
        let mut node_id = start_data.node_id;
        let last_node_id = start_data.last_node_id;

        visited.visit_edge(link_id, visited_state);

        let last_link_id =
            GraphUtil::next_link(self.links, self.nodes, link_id, last_node_id, !clockwise, visited);

        // Find a closed tour
        while link_id != last_link_id {
            link_id = GraphUtil::next_link(self.links, self.nodes, link_id, node_id, clockwise, visited);

            let link = unsafe {
                // Safety: `link_id` is always derived from a previous in-bounds index or
                // from `find_left_top_link`, so it remains in `0..self.links.len()`.
                self.links.get_unchecked(link_id)
            };

            node_id = if link.a.id == node_id {
                link.b.id
            } else {
                link.a.id
            };

            visited.visit_edge(link_id, visited_state);
        }
    }

    fn collect_shape(
        &self,
        start_data: &StartPathData<I>,
        clockwise: bool,
        global_visited: &mut [VisitState],
        contour_visited: &mut [VisitState],
        points: &mut Vec<IntPoint<I>>,
    ) -> Option<IntShape<I>> {
        let mut link_id = start_data.link_id;
        let mut node_id = start_data.node_id;
        let last_node_id = start_data.last_node_id;

        // First, mark all edges that belong to the contour.

        let mut end_link_id = start_data.link_id;

        global_visited.visit_edge(link_id, VisitState::HullVisited);
        contour_visited.visit_edge(link_id, VisitState::Unvisited);

        let last_link_id = GraphUtil::next_link(
            self.links,
            self.nodes,
            link_id,
            last_node_id,
            !clockwise,
            global_visited,
        );

        let mut original_contour_len = 1;

        // Find a closed tour
        while link_id != last_link_id {
            link_id = GraphUtil::next_link(
                self.links,
                self.nodes,
                link_id,
                node_id,
                clockwise,
                global_visited,
            );

            let link = unsafe {
                // Safety: `link_id` is always derived from a previous in-bounds index or
                // from `find_left_top_link`, so it remains in `0..self.links.len()`.
                self.links.get_unchecked(link_id)
            };

            node_id = if link.a.id == node_id {
                link.b.id
            } else {
                link.a.id
            };
            end_link_id = end_link_id.max(link_id);
            contour_visited.visit_edge(link_id, VisitState::Unvisited);
            global_visited.visit_edge(link_id, VisitState::HullVisited);
            original_contour_len += 1;
        }

        // Revisit the contour in reverse;
        // all links escape current contour are skipped in `contour_visited`.

        points.reserve_capacity(original_contour_len);
        self.find_contour(
            start_data,
            !clockwise,
            VisitState::HullVisited,
            contour_visited,
            points,
        );

        let (is_valid, _) = points.validate(
            self.options.min_output_area,
            self.options.preserve_output_collinear,
        );

        let contour_len = points.len();

        let mut shape = if is_valid {
            let mut shape = vec![];
            let contour = points.as_slice().to_vec();
            shape.push(contour);
            Some(shape)
        } else {
            None
        };

        if contour_len < original_contour_len {
            // contour has self touches
            let mut link_index = start_data.link_id;
            while link_index <= end_link_id {
                if contour_visited.is_visited(link_index) {
                    link_index += 1;
                    continue;
                }

                let left_top_link = unsafe {
                    // Safety: `link_index` walks 0..buffer.visited.len(), and buffer.visited.len() <= self.links.len().
                    GraphUtil::find_left_top_link(self.links, self.nodes, link_index, contour_visited)
                };

                let link = unsafe {
                    // Safety: `left_top_link` originates from `find_left_top_link`, which only returns
                    // indices in 0..self.links.len(), so this lookup cannot go out of bounds.
                    self.links.get_unchecked(left_top_link)
                };

                // Self-touch splits can only produce holes inside this contour.

                let hole_start_data = StartPathData::new(clockwise, link, left_top_link);
                self.find_contour(
                    &hole_start_data,
                    clockwise,
                    VisitState::HoleVisited,
                    contour_visited,
                    points,
                );

                // Hole have to belong to this shape.
                if let Some(shape) = shape.as_mut() {
                    let (is_valid, _) = points.validate(
                        self.options.min_output_area,
                        self.options.preserve_output_collinear,
                    );

                    if !is_valid {
                        link_index += 1;
                        continue;
                    }

                    let contour = points.as_slice().to_vec();
                    shape.push(contour);
                }
            }
        }

        shape
    }
}
