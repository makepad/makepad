use crate::float::overlay::OverlayOptions;
use crate::string::graph::StringGraph;
use crate::string::rule::StringRule;
use i_float::adapter::FloatPointAdapter;
use i_float::float::compatible::FloatPointCompatible;
use i_float::int::number::int::IntNumber;
use i_key_sort::sort::key::SortKey;
use i_shape::base::data::Shapes;
use i_shape::float::adapter::ShapesToFloat;
use i_shape::float::despike::DeSpikeContour;
use i_shape::float::simple::SimplifyContour;
use i_tree::{Expiration, LayoutNumber};

/// The `FloatStringGraph` struct represents a graph structure with floating-point precision,
/// providing methods to extract geometric shapes from the graph after applying string-based operations.
pub struct FloatStringGraph<'a, P: FloatPointCompatible, I: IntNumber = i32> {
    pub graph: StringGraph<'a, I>,
    pub adapter: FloatPointAdapter<P, I>,
}

impl<P, I> FloatStringGraph<'_, P, I>
where
    P: FloatPointCompatible,
    I: IntNumber + Expiration + LayoutNumber + SortKey,
{
    /// Extracts shapes from the overlay graph based on the specified string rule.
    /// This method is used to retrieve the final geometric shapes after boolean operations have been applied.
    /// It's suitable for most use cases where the minimum area of shapes is not a concern.
    ///
    /// # Parameters
    /// - `string_rule`: The string operation rule to apply when extracting shapes from the graph, such as slice.
    ///
    /// # Returns
    /// A `Shapes<P>` collection, representing the geometric result of the applied string rule.
    ///
    /// # Shape Representation
    /// The output is a `Shapes<P>`, where:
    /// - The outer `Shapes<P>` represents a set of shapes.
    /// - Each shape `Shape<P>` represents a collection of paths, where the first path is the outer boundary, and all subsequent paths are holes in this boundary.
    /// - Each path `Contour<P>` is a sequence of points, forming a closed path.
    ///
    /// Note: Outer boundary paths have a counterclockwise order, and holes have a clockwise order.
    #[inline(always)]
    pub fn extract_shapes(&self, string_rule: StringRule) -> Shapes<P> {
        self.extract_shapes_custom(string_rule, Default::default())
    }

    /// Extracts shapes from the overlay graph similar to `extract_shapes`, but with an additional constraint on the minimum area of the shapes.
    /// This is useful for filtering out shapes that do not meet a certain size threshold, which can be beneficial for eliminating artifacts or noise from the output.
    ///
    /// # Parameters
    /// - `string_rule`: The string operation rule to apply when extracting shapes from the graph, such as slice.
    /// - `options`: Adjust custom behavior.
    ///
    /// # Returns
    /// A `Shapes<P>` collection, representing the geometric result of the applied string rule.
    ///
    /// # Shape Representation
    /// The output is a `Shapes<P>`, where:
    /// - The outer `Shapes<P>` represents a set of shapes.
    /// - Each shape `Shape<P>` represents a collection of paths, where the first path is the outer boundary, and all subsequent paths are holes in this boundary.
    /// - Each path `Contour<P>` is a sequence of points, forming a closed path.
    ///
    /// Note: Outer boundary paths have a **main_direction** order, and holes have an opposite to **main_direction** order.
    #[inline]
    pub fn extract_shapes_custom(
        &self,
        string_rule: StringRule,
        options: OverlayOptions<P::Scalar, I>,
    ) -> Shapes<P> {
        let shapes = self
            .graph
            .extract_shapes_custom(string_rule, options.int_with_adapter(&self.adapter));
        let mut float = shapes.to_float(&self.adapter);

        if options.clean_result {
            if options.preserve_output_collinear {
                float.despike_contour(&self.adapter);
            } else {
                float.simplify_contour(&self.adapter);
            }
        }

        float
    }
}
