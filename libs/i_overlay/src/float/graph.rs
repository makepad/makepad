//! This module defines the graph structure that represents the relationships between the paths in
//! subject and clip polygons after boolean operations. The graph helps in extracting final shapes
//! based on the overlay rule applied.

use crate::core::extract::BooleanExtractionBuffer;
use crate::core::graph::OverlayGraph;
use crate::core::overlay_rule::OverlayRule;
use i_float::adapter::FloatPointAdapter;
use i_float::float::compatible::FloatPointCompatible;
use i_float::int::number::int::IntNumber;
use i_key_sort::sort::key::SortKey;
use i_shape::base::data::Shapes;
use i_shape::float::adapter::ShapesToFloat;
use i_shape::float::despike::DeSpikeContour;
use i_shape::float::simple::SimplifyContour;
use i_tree::{Expiration, LayoutNumber};

/// The `FloatOverlayGraph` struct represents an overlay graph with floating point precision,
/// providing methods to extract geometric shapes from the graph after applying boolean operations.
/// [More information](https://ishape-rust.github.io/iShape-js/overlay/overlay_graph/overlay_graph.html) about Overlay Graph.
pub struct FloatOverlayGraph<'a, P: FloatPointCompatible, I: IntNumber = i32> {
    pub graph: OverlayGraph<'a, I>,
    pub adapter: FloatPointAdapter<P, I>,
    clean_result: bool,
}

impl<'a, P, I> FloatOverlayGraph<'a, P, I>
where
    P: FloatPointCompatible,
    I: IntNumber + Expiration + LayoutNumber + SortKey,
{
    #[inline]
    pub(crate) fn new(
        graph: OverlayGraph<'a, I>,
        adapter: FloatPointAdapter<P, I>,
        clean_result: bool,
    ) -> Self {
        Self {
            graph,
            adapter,
            clean_result,
        }
    }

    /// Extracts shapes from the overlay graph based on the specified overlay rule.
    /// This method is used to retrieve the final geometric shapes after boolean operations have been applied.
    /// It's suitable for most use cases where the minimum area of shapes is not a concern.
    ///
    /// # Parameters
    /// - `overlay_rule`: The boolean operation rule to apply when extracting shapes from the graph, such as union or intersection.
    ///
    /// # Returns
    /// A `Shapes<P>` collection, representing the geometric result of the applied overlay rule.
    ///
    /// # Shape Representation
    /// The output is a `Shapes<P>`, where:
    /// - The outer `Vec<Shape<P>>` represents a set of shapes.
    /// - Each shape `Vec<Contour<P>>` represents a collection of paths, where the first path is the outer boundary, and all subsequent paths are holes in this boundary.
    /// - Each path `Vec<P>` is a sequence of points, forming a closed path.
    ///
    /// Note: Outer boundary paths have a counterclockwise order, and holes have a clockwise order.
    #[inline]
    pub fn extract_shapes(
        &self,
        overlay_rule: OverlayRule,
        buffer: &mut BooleanExtractionBuffer<I>,
    ) -> Shapes<P> {
        let shapes = self.graph.extract_shapes(overlay_rule, buffer);
        let mut float = shapes.to_float(&self.adapter);

        if self.clean_result {
            if self.graph.options.preserve_output_collinear {
                float.despike_contour(&self.adapter);
            } else {
                float.simplify_contour(&self.adapter);
            }
        }

        float
    }
}
