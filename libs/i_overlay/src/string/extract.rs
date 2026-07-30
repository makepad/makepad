use crate::bind::solver::JoinHoles;
use crate::core::nearest_vector::NearestVector;
use crate::core::overlay::{ContourDirection, IntOverlayOptions};
use crate::segm::segment::SUBJ_TOP;
use crate::string::graph::StringGraph;
use crate::string::rule::StringRule;
use crate::string::split::{BinStore, Split};
use alloc::vec;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_key_sort::sort::key::SortKey;
use i_shape::int::path::{ContourExtension, IntPath};
use i_shape::int::shape::IntShapes;
use i_tree::Expiration;

impl<I: IntNumber + Expiration + SortKey> StringGraph<'_, I> {
    /// Extracts shapes from the graph based on the specified `StringRule`.
    /// - `string_rule`: The rule used to determine how shapes are extracted.
    /// # Shape Representation
    /// The output is a `IntShapes<I>`, where:
    /// - The outer `Vec<IntShape<I>>` represents a set of shapes.
    /// - Each shape `Vec<IntContour<I>>` represents a collection of contours, where the first contour is the outer boundary, and all subsequent contours are holes in this boundary.
    /// - Each path `Vec<IntPoint>` is a sequence of points, forming a closed path.
    ///
    /// Note: Outer boundary paths have a counterclockwise order, and holes have a clockwise order.
    #[inline(always)]
    pub fn extract_shapes(&self, string_rule: StringRule) -> IntShapes<I> {
        self.extract_shapes_custom(string_rule, Default::default())
    }

    /// Extracts shapes from the graph with a minimum area constraint.
    /// - `string_rule`: The rule used to determine how shapes are extracted.
    /// - `main_direction`: Winding direction for the **output** main (outer) contour. All hole contours will automatically use the opposite direction. Impact on **output** only!
    /// - `min_area`: The minimum area that a shape must have to be included in the results. Shapes smaller than this will be excluded.
    /// - Returns: A vector of `IntShape<I>`, representing the geometric result of the applied overlay rule.
    /// # Shape Representation
    /// The output is a `IntShapes<I>`, where:
    /// - The outer `Vec<IntShape<I>>` represents a set of shapes.
    /// - Each shape `Vec<IntContour<I>>` represents a collection of contours, where the first contour is the outer boundary, and all subsequent contours are holes in this boundary.
    /// - Each path `Vec<IntPoint>` is a sequence of points, forming a closed path.
    ///
    /// Note: Outer boundary paths have a **main_direction** order, and holes have an opposite to **main_direction** order.
    pub fn extract_shapes_custom(
        &self,
        string_rule: StringRule,
        options: IntOverlayOptions<I::WideUInt>,
    ) -> IntShapes<I> {
        let clockwise = options.output_direction == ContourDirection::Clockwise;
        let mut fills = self.filter(string_rule);
        let mut shapes = Vec::new();
        let mut holes = Vec::new();

        let mut contour_buffer = Vec::new();
        let mut bin_store = BinStore::new();

        let mut link_index = 0;
        while link_index < fills.len() {
            let fill = fills[link_index];
            if fill == 0 {
                link_index += 1;
                continue;
            }

            let direction = fill & SUBJ_TOP == SUBJ_TOP;
            let paths = self.get_paths(link_index, direction, &mut fills).split_loops(
                options.min_output_area,
                &mut contour_buffer,
                &mut bin_store,
            );

            for mut path in paths.into_iter() {
                let order = path.is_clockwise_ordered();
                let is_hole = order == direction;
                if is_hole {
                    if clockwise == order {
                        // clockwise == direction
                        path.reverse();
                    }
                    holes.push(path);
                } else {
                    if clockwise != order {
                        path.reverse();
                    }
                    shapes.push(vec![path]);
                }
            }
        }

        shapes.join_unsorted_holes(holes, clockwise);

        shapes
    }

    #[inline]
    fn get_paths(&self, start_index: usize, clockwise: bool, fills: &mut [u8]) -> IntPath<I> {
        let start_link = unsafe {
            // SAFETY: start_index originates from iterating the fills array, which mirrors links.
            self.links.get_unchecked(start_index)
        };

        let mut link_id = start_index;
        let mut node_id = start_link.b.id;
        let last_node_id = start_link.a.id;

        let mut path = IntPath::<I>::new();
        path.push(start_link.a.point);

        fills[start_index] = start_link.visit_fill(fills[start_index], start_link.a.id, clockwise);

        // Find a closed tour
        while node_id != last_node_id {
            link_id = self.find_nearest_link_to(link_id, node_id, clockwise, fills);

            let link = unsafe {
                // SAFETY: link_id comes from find_nearest_link_to, which only yields valid link indices.
                self.links.get_unchecked(link_id)
            };
            fills[link_id] = link.visit_fill(fills[link_id], node_id, clockwise);

            node_id = if link.a.id == node_id {
                path.push(link.a.point);
                link.b.id
            } else {
                path.push(link.b.point);
                link.a.id
            };
        }

        path
    }

    fn find_nearest_link_to(
        &self,
        target_index: usize,
        node_id: usize,
        clockwise: bool,
        fills: &[u8],
    ) -> usize {
        let indices = unsafe {
            // SAFETY: node_id comes from an endpoint of an existing link, so it indexes nodes.
            self.nodes.get_unchecked(node_id)
        };
        let mut is_first = true;
        let mut first_index = usize::MAX;
        let mut second_index = usize::MAX;
        let mut pos = 0;
        for (i, &link_index) in indices.iter().enumerate() {
            if link_index == target_index {
                continue;
            }
            let (link, fill) = unsafe {
                // SAFETY: link_index comes from the adjacency list; fills shares the same length as links.
                (
                    self.links.get_unchecked(link_index),
                    *fills.get_unchecked(link_index),
                )
            };
            if link.is_move_possible(fill, node_id, clockwise) {
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

        if first_index == usize::MAX {
            let (link, fill) = unsafe {
                // SAFETY: target_index is the caller's current link id, so it is within bounds for both arrays.
                (
                    self.links.get_unchecked(target_index),
                    *fills.get_unchecked(target_index),
                )
            };
            if link.is_move_possible(fill, node_id, clockwise) {
                return target_index;
            } else {
                panic!("no move found")
            }
        }

        if second_index == usize::MAX {
            return first_index;
        }

        let target = unsafe {
            // SAFETY: target_index is either target_index or first_index; both were validated above.
            self.links.get_unchecked(target_index)
        };
        let (c, a) = if target.a.id == node_id {
            (target.a.point, target.b.point)
        } else {
            (target.b.point, target.a.point)
        };

        // more the one vectors
        let b = unsafe {
            // SAFETY: first_index came from indices, so it indexes links.
            self.links.get_unchecked(first_index)
        }
        .other(node_id)
        .point;
        let mut vector_solver = NearestVector::new(c, a, b, first_index, clockwise);

        // add second vector
        let point = unsafe {
            // SAFETY: second_index is also sourced from indices, matching links' bounds.
            self.links.get_unchecked(second_index)
        }
        .other(node_id)
        .point;
        vector_solver.add(point, second_index);

        // check the rest vectors
        for &link_index in indices.iter().skip(pos + 1) {
            let (link, fill) = unsafe {
                // SAFETY: link_index traverses the same adjacency slice; indices and arrays are aligned.
                (
                    self.links.get_unchecked(link_index),
                    *fills.get_unchecked(link_index),
                )
            };
            if link.is_move_possible(fill, node_id, clockwise) {
                let p = link.other(node_id).point;
                vector_solver.add(p, link_index);
            }
        }

        vector_solver.best_id
    }
}

#[cfg(test)]
mod tests {
    use crate::core::fill_rule::FillRule;
    use crate::string::overlay::StringOverlay;
    use crate::string::rule::StringRule;
    use alloc::vec;
    use i_float::int::point::IntPoint;

    #[test]
    fn test_0() {
        let paths = vec![vec![
            IntPoint::new(-10, 10),
            IntPoint::new(-10, -10),
            IntPoint::new(10, -10),
            IntPoint::new(10, 10),
        ]];

        let window = vec![
            IntPoint::new(-5, -5),
            IntPoint::new(-5, 5),
            IntPoint::new(5, 5),
            IntPoint::new(5, -5),
        ];

        let mut overlay = StringOverlay::with_shape(&paths);
        overlay.add_string_contour(&window);
        let graph = overlay.build_graph_view(FillRule::NonZero).unwrap();

        let r = graph.extract_shapes_custom(StringRule::Slice, Default::default());

        assert_eq!(r.len(), 2);
    }

    #[test]
    fn test_i64_overlay() {
        let paths = vec![vec![
            IntPoint::<i64>::new(-10, 10),
            IntPoint::new(-10, -10),
            IntPoint::new(10, -10),
            IntPoint::new(10, 10),
        ]];

        let window = vec![
            IntPoint::new(-5, -5),
            IntPoint::new(-5, 5),
            IntPoint::new(5, 5),
            IntPoint::new(5, -5),
        ];

        let mut overlay = StringOverlay::<i64>::with_shape(&paths);
        overlay.add_string_contour(&window);
        let graph = overlay.build_graph_view(FillRule::NonZero).unwrap();

        let r = graph.extract_shapes_custom(StringRule::Slice, Default::default());

        assert_eq!(r.len(), 2);
    }
}
