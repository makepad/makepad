use super::overlay_rule::OverlayRule;
use crate::bind::segment::{ContourIndex, IdSegment};
use crate::bind::solver::{JoinHoles, LeftBottomSegment};
use crate::core::graph::{OverlayGraph, OverlayNode};
use crate::core::link::OverlayLink;
use crate::core::link::OverlayLinkFilter;
use crate::core::nearest_vector::NearestVector;
use crate::core::overlay::ContourDirection;
use crate::i_shape::flat::buffer::FlatContoursBuffer;
use alloc::vec;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_float::int::number::uint::UIntNumber;
use i_float::int::number::wide_int::WideIntNumber;
use i_float::int::point::IntPoint;
use i_float::triangle::Triangle;
use i_key_sort::sort::key::SortKey;
use i_shape::int::path::ContourExtension;
use i_shape::int::shape::{IntContour, IntShapes};
use i_shape::int::simple::Simplify;
use i_shape::util::reserve::Reserve;
use i_tree::Expiration;

#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Default)]
pub(crate) enum VisitState {
    #[default]
    Unvisited = 0,
    Skipped = 1,
    HoleVisited = 2,
    HullVisited = 3,
}

pub struct BooleanExtractionBuffer<I: IntNumber> {
    pub(crate) points: Vec<IntPoint<I>>,
    pub(crate) visited: Vec<VisitState>,
    pub(crate) contour_visited: Option<Vec<VisitState>>,
}

impl<I: IntNumber> Default for BooleanExtractionBuffer<I> {
    fn default() -> Self {
        Self {
            points: Vec::new(),
            visited: Vec::new(),
            contour_visited: None,
        }
    }
}

impl<I> OverlayGraph<'_, I>
where
    I: IntNumber + Expiration + SortKey,
{
    /// Extracts shapes from the overlay graph based on the specified overlay rule. This method is used to retrieve the final geometric shapes after boolean operations have been applied. It's suitable for most use cases where the minimum area of shapes is not a concern.
    /// - `overlay_rule`: The boolean operation rule to apply when extracting shapes from the graph, such as union or intersection.
    /// - `buffer`: Reusable buffer, optimisation purpose only.
    /// - Returns: A vector of `IntShape<I>`, representing the geometric result of the applied overlay rule.
    /// # Shape Representation
    /// The output is a `IntShapes<I>`, where:
    /// - The outer `Vec<IntShape<I>>` represents a set of shapes.
    /// - Each shape `Vec<IntContour<I>>` represents a collection of contours, where the first contour is the outer boundary, and all subsequent contours are holes in this boundary.
    /// - Each path `Vec<IntPoint>` is a sequence of points, forming a closed path.
    ///
    /// Note: Outer boundary paths have a counterclockwise order, and holes have a clockwise order.
    #[inline]
    pub fn extract_shapes(
        &self,
        overlay_rule: OverlayRule,
        buffer: &mut BooleanExtractionBuffer<I>,
    ) -> IntShapes<I> {
        self.links
            .filter_by_overlay_into(overlay_rule, &mut buffer.visited);
        if self.options.ogc {
            self.extract_ogc(overlay_rule, buffer)
        } else {
            self.extract(overlay_rule, buffer)
        }
    }

    /// Extracts the flat contours from the overlay graph based on the specified overlay rule.
    ///
    /// This method performs a Boolean operation (e.g., union or intersection) and stores the result
    /// directly into a flat buffer of contours, without nesting them into shapes (i.e., no hole-joining or grouping).
    ///
    /// It is optimized for performance and suitable when raw contour data is sufficient,
    /// such as during intermediate processing, visualization, or tesselation.
    ///
    /// - `overlay_rule`: The boolean operation rule to apply (e.g., union, intersection, xor).
    /// - `buffer`: Reusable working buffer to avoid reallocations.
    /// - `output`: A flat buffer to which the resulting valid contours will be written.
    #[inline]
    pub fn extract_contours_into(
        &self,
        overlay_rule: OverlayRule,
        buffer: &mut BooleanExtractionBuffer<I>,
        output: &mut FlatContoursBuffer<I>,
    ) {
        self.links
            .filter_by_overlay_into(overlay_rule, &mut buffer.visited);
        self.extract_contours(overlay_rule, buffer, output);
    }

    pub(crate) fn extract(
        &self,
        overlay_rule: OverlayRule,
        buffer: &mut BooleanExtractionBuffer<I>,
    ) -> IntShapes<I> {
        let clockwise = self.options.output_direction == ContourDirection::Clockwise;

        let mut shapes = Vec::new();
        let mut holes = Vec::new();
        let mut anchors = Vec::new();

        buffer.points.reserve_capacity(buffer.visited.len());

        let mut link_index = 0;
        let mut anchors_already_sorted = true;
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

            let direction = is_hole == clockwise;
            let start_data = StartPathData::new(direction, link, left_top_link);

            self.find_contour(
                &start_data,
                direction,
                visited_state,
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

            if is_hole {
                let left_bottom = if clockwise { contour[1] } else { contour[0] };
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
            } else {
                shapes.push(vec![contour]);
            }
        }

        if !anchors_already_sorted {
            anchors.sort_unstable_by_key(|s0| s0.v_segment.a);
        }

        shapes.join_sorted_holes(holes, anchors, clockwise);

        shapes
    }

    pub(crate) fn find_contour(
        &self,
        start_data: &StartPathData<I>,
        clockwise: bool,
        visited_state: VisitState,
        visited: &mut [VisitState],
        points: &mut Vec<IntPoint<I>>,
    ) {
        let mut link_id = start_data.link_id;
        let mut node_id = start_data.node_id;
        let last_node_id = start_data.last_node_id;

        visited.visit_edge(link_id, visited_state);
        points.clear();
        points.push(start_data.begin);

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
            node_id = points.push_node_and_get_other(link, node_id);

            visited.visit_edge(link_id, visited_state);
        }
    }

    fn extract_contours(
        &self,
        overlay_rule: OverlayRule,
        buffer: &mut BooleanExtractionBuffer<I>,
        output: &mut FlatContoursBuffer<I>,
    ) {
        let clockwise = self.options.output_direction == ContourDirection::Clockwise;
        let len = buffer.visited.len();
        buffer.points.reserve_capacity(len);
        output.clear_and_reserve(len, 4);

        let mut link_index = 0;
        while link_index < len {
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

            let direction = is_hole == clockwise;
            let start_data = StartPathData::new(direction, link, left_top_link);

            self.find_contour(
                &start_data,
                direction,
                visited_state,
                &mut buffer.visited,
                &mut buffer.points,
            );
            let (is_valid, _) = buffer.points.validate(
                self.options.min_output_area,
                self.options.preserve_output_collinear,
            );

            if !is_valid {
                link_index += 1;
                continue;
            }

            output.add_contour(buffer.points.as_slice());
        }
    }
}

pub(crate) struct StartPathData<I: IntNumber> {
    pub(crate) begin: IntPoint<I>,
    pub(crate) node_id: usize,
    pub(crate) link_id: usize,
    pub(crate) last_node_id: usize,
}

impl<I: IntNumber> StartPathData<I> {
    #[inline(always)]
    pub(crate) fn new(direction: bool, link: &OverlayLink<I>, link_id: usize) -> Self {
        if direction {
            Self {
                begin: link.b.point,
                node_id: link.a.id,
                link_id,
                last_node_id: link.b.id,
            }
        } else {
            Self {
                begin: link.a.point,
                node_id: link.b.id,
                link_id,
                last_node_id: link.a.id,
            }
        }
    }
}

pub(crate) trait GraphContour<I: IntNumber> {
    fn validate(&mut self, min_output_area: I::WideUInt, preserve_output_collinear: bool) -> (bool, bool);
    fn push_node_and_get_other<D>(&mut self, link: &OverlayLink<I, D>, node_id: usize) -> usize;
}

impl<I: IntNumber> GraphContour<I> for IntContour<I> {
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
        let area = self.unsafe_area();
        let abs_area = area.unsigned_abs() >> 1;
        let is_valid = abs_area >= min_output_area;

        (is_valid, is_modified)
    }

    #[inline]
    fn push_node_and_get_other<D>(&mut self, link: &OverlayLink<I, D>, node_id: usize) -> usize {
        if link.a.id == node_id {
            self.push(link.a.point);
            link.b.id
        } else {
            self.push(link.b.point);
            link.a.id
        }
    }
}

impl VisitState {
    #[inline(always)]
    pub(crate) fn new(skipped: bool) -> Self {
        let raw = skipped as u8; // 0 or 1
        debug_assert!(raw <= VisitState::Skipped as u8);
        // SAFETY: repr(u8) and raw is in range 0..=1
        unsafe { core::mem::transmute(raw) }
    }
}

pub(crate) trait Visit {
    fn is_visited(&self, index: usize) -> bool;
    fn is_not_visited(&self, index: usize) -> bool;
    fn visit_edge(&mut self, index: usize, state: VisitState);
}

// Safety: every call site creates `visited` slices with one entry per link/node,
// and they only pass indices directly obtained from those slices. That keeps
// `index < self.len()` true for the lifetime of the traversal.
impl Visit for [VisitState] {
    #[inline(always)]
    fn is_visited(&self, index: usize) -> bool {
        unsafe {
            // SAFETY: callers only pass indices derived from the visited slice itself, so index < len.
            *self.get_unchecked(index) != VisitState::Unvisited
        }
    }

    #[inline(always)]
    fn is_not_visited(&self, index: usize) -> bool {
        unsafe {
            // SAFETY: callers only pass indices derived from the visited slice itself, so index < len.
            *self.get_unchecked(index) == VisitState::Unvisited
        }
    }
    #[inline(always)]
    fn visit_edge(&mut self, index: usize, state: VisitState) {
        unsafe {
            // SAFETY: callers only pass indices derived from the visited slice itself, so index < len.
            *self.get_unchecked_mut(index) = state;
        }
    }
}

pub(crate) struct GraphUtil;

impl GraphUtil {
    /// # Safety
    /// * `link_index < links.len()`
    /// * `links[top.a.id]` must exist for every link
    /// * For bridge nodes, both `bridge[k] < links.len()`
    /// * `visited` is at least `links.len()` long (or whatever invariant applies)
    #[inline]
    pub(crate) unsafe fn find_left_top_link<I: IntNumber, D>(
        links: &[OverlayLink<I, D>],
        nodes: &[OverlayNode],
        link_index: usize,
        visited: &[VisitState],
    ) -> usize {
        let top = unsafe {
            // SAFETY: link_index is always < links.len(); callers either iterate that range or
            // pull the value from visited, which mirrors links.
            links.get_unchecked(link_index)
        };
        let node = unsafe {
            // SAFETY: GraphBuilder assigns `a.id` from the index in the `nodes` vector,
            // so every `id` is guaranteed to lie in 0..nodes.len().
            nodes.get_unchecked(top.a.id)
        };

        debug_assert!(top.is_direct());

        match node {
            OverlayNode::Bridge(bridge) => Self::find_left_top_link_on_bridge(links, bridge),
            OverlayNode::Cross(indices) => {
                Self::find_left_top_link_on_indices(links, top, link_index, indices, visited)
            }
        }
    }

    #[inline(always)]
    fn find_left_top_link_on_indices<I: IntNumber, D>(
        links: &[OverlayLink<I, D>],
        link: &OverlayLink<I, D>,
        link_index: usize,
        indices: &[usize],
        visited: &[VisitState],
    ) -> usize {
        let mut top_index = link_index;
        let mut top = link;

        // find most top link

        for &i in indices.iter() {
            if i == link_index {
                continue;
            }
            let link = unsafe {
                // SAFETY: indices holds link ids emitted by GraphBuilder, so each i < links.len().
                links.get_unchecked(i)
            };
            if !link.is_direct() || Triangle::is_clockwise(top.a.point, top.b.point, link.b.point) {
                continue;
            }

            if visited.is_visited(i) {
                continue;
            }

            top_index = i;
            top = link;
        }

        top_index
    }

    #[inline(always)]
    fn find_left_top_link_on_bridge<I: IntNumber, D>(
        links: &[OverlayLink<I, D>],
        bridge: &[usize; 2],
    ) -> usize {
        // SAFETY: every bridge index comes straight from GraphBuilder::build_nodes_and_connect_links,
        // which only records values in 0..links.len(), so the unchecked lookups stay in-bounds.
        let (l0, l1) = unsafe { (links.get_unchecked(bridge[0]), links.get_unchecked(bridge[1])) };
        if Triangle::is_clockwise(l0.a.point, l0.b.point, l1.b.point) {
            bridge[0]
        } else {
            bridge[1]
        }
    }

    #[inline(always)]
    pub(crate) fn next_link<I: IntNumber, D>(
        links: &[OverlayLink<I, D>],
        nodes: &[OverlayNode],
        link_id: usize,
        node_id: usize,
        clockwise: bool,
        visited: &[VisitState],
    ) -> usize {
        let node = unsafe {
            // SAFETY: all node ids flowing through traversal originate from GraphBuilder,
            // hence are within `0..nodes.len()`.
            nodes.get_unchecked(node_id)
        };
        match node {
            OverlayNode::Bridge(bridge) => {
                if bridge[0] == link_id {
                    bridge[1]
                } else {
                    bridge[0]
                }
            }
            OverlayNode::Cross(indices) => {
                GraphUtil::find_nearest_link_to(links, link_id, node_id, clockwise, indices, visited)
            }
        }
    }

    // Assumes: `indices` comes from an OverlayNode::Cross built by GraphBuilder,
    // so every element is a valid index into `links`, and at least one of them is
    // still unvisited when we enter. The unchecked accesses rely on that invariant.
    #[inline]
    fn find_nearest_link_to<I: IntNumber, D>(
        links: &[OverlayLink<I, D>],
        target_index: usize,
        node_id: usize,
        clockwise: bool,
        indices: &[usize],
        visited: &[VisitState],
    ) -> usize {
        let mut is_first = true;
        let mut first_index = 0;
        let mut second_index = usize::MAX;
        let mut pos = 0;
        for (i, &link_index) in indices.iter().enumerate() {
            if visited.is_not_visited(link_index) {
                if is_first {
                    first_index = link_index;
                    is_first = false;
                } else {
                    second_index = link_index;
                    pos = i;
                    break;
                }
            }
        }

        if second_index == usize::MAX {
            return first_index;
        }

        let target = unsafe {
            // SAFETY: target_index is either the incoming link_id or an entry from indices, both validated.
            links.get_unchecked(target_index)
        };
        let (c, a) = if target.a.id == node_id {
            (target.a.point, target.b.point)
        } else {
            (target.b.point, target.a.point)
        };

        // more the one vectors
        let b = unsafe {
            // SAFETY: first_index originates from indices, so it is within links.
            links.get_unchecked(first_index)
        }
        .other(node_id)
        .point;
        let mut vector_solver = NearestVector::new(c, a, b, first_index, clockwise);

        // add second vector
        vector_solver.add(
            unsafe {
                // SAFETY: second_index comes from indices just like first_index.
                links.get_unchecked(second_index)
            }
            .other(node_id)
            .point,
            second_index,
        );

        // check the rest vectors
        for &link_index in indices.iter().skip(pos + 1) {
            if visited.is_not_visited(link_index) {
                let p = unsafe {
                    // SAFETY: every link_index here is sourced from indices, so it addresses links.
                    links.get_unchecked(link_index)
                }
                .other(node_id)
                .point;
                vector_solver.add(p, link_index);
            }
        }

        vector_solver.best_id
    }
}

#[cfg(test)]
mod tests {
    use crate::core::fill_rule::FillRule;
    use crate::core::overlay::{ContourDirection, Overlay};
    use crate::core::overlay_rule::OverlayRule;
    use i_shape::int_shape;

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
            .extract_shapes(OverlayRule::Subject, &mut buffer);

        debug_assert!(shapes_0.len() == 1);

        overlay.options.output_direction = ContourDirection::Clockwise;
        let shapes_1 = overlay
            .build_graph_view(FillRule::NonZero)
            .unwrap()
            .extract_shapes(OverlayRule::Subject, &mut buffer);

        debug_assert!(shapes_1.len() == 1);
    }
}
