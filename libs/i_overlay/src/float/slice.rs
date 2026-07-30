use crate::core::fill_rule::FillRule;
use crate::core::solver::Solver;
use crate::float::overlay::OverlayOptions;
use crate::float::scale::FixedScaleOverlayError;
use crate::float::string_overlay::FloatStringOverlay;
use crate::string::rule::StringRule;
use i_float::float::compatible::FloatPointCompatible;
use i_float::int::number::int::IntNumber;
use i_key_sort::sort::key::SortKey;
use i_shape::base::data::Shapes;
use i_shape::source::resource::ShapeResource;
use i_tree::{Expiration, LayoutNumber};

/// The `FloatSlice` trait provides methods to slice geometric shapes using a given path or set of paths,
/// allowing for boolean operations based on the specified build rule.
///
/// This convenience trait uses the default integer engine (`i32`). Use the `*_as::<I>` methods
/// when you need to select `i16`, `i32`, or `i64` explicitly.
///
/// # Example
///
/// ```
/// use i_overlay::core::fill_rule::FillRule;
/// use i_overlay::float::slice::FloatSlice;
///
/// let shape = vec![[0.0, 0.0], [0.0, 2.0], [2.0, 2.0], [2.0, 0.0]];
/// let line = vec![[-1.0, 1.0], [3.0, 1.0]];
///
/// let result = shape.slice_by_as::<i64>(&line, FillRule::EvenOdd);
///
/// assert_eq!(result.len(), 2);
/// ```
pub trait FloatSlice<R, P>
where
    R: ShapeResource<P>,
    P: FloatPointCompatible,
{
    /// Slices the current shapes by string lines.
    ///
    /// - `resource`: A string lines.
    ///   `ShapeResource` can be one of the following:
    ///     - `Path`: A path representing a string line.
    ///     - `Paths`: A collection of paths, each representing a string line.
    ///     - `Vec<Paths>`: A collection of grouped paths, where each group may consist of multiple paths.
    /// - `fill_rule`: Fill rule to determine filled areas (non-zero, even-odd, positive, negative).
    ///
    /// Returns a `Shapes<P>` collection representing the sliced geometry.
    ///
    /// Note: Outer boundary paths have a counterclockwise order, and holes have a clockwise order.
    fn slice_by(&self, resource: &R, fill_rule: FillRule) -> Shapes<P>;

    /// Same as [`Self::slice_by`], but with an explicit integer engine.
    fn slice_by_as<I>(&self, resource: &R, fill_rule: FillRule) -> Shapes<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey;

    /// Slices the current shapes by string lines with a fixed float-to-integer scale.
    ///
    /// - `resource`: A string lines.
    ///   `ShapeResource` can be one of the following:
    ///     - `Path`: A path representing a string line.
    ///     - `Paths`: A collection of paths, each representing a string line.
    ///     - `Vec<Paths>`: A collection of grouped paths, where each group may consist of multiple paths.
    /// - `fill_rule`: Fill rule to determine filled areas (non-zero, even-odd, positive, negative).
    /// - `scale`: Fixed float-to-integer scale. Use `scale = 1.0 / grid_size` if you prefer grid size semantics.
    ///
    /// Returns a `Shapes<P>` collection representing the sliced geometry.
    ///
    /// Note: Outer boundary paths have a counterclockwise order, and holes have a clockwise order.
    fn slice_by_fixed_scale(
        &self,
        resource: &R,
        fill_rule: FillRule,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError>;

    /// Same as [`Self::slice_by_fixed_scale`], but with an explicit integer engine.
    fn slice_by_fixed_scale_as<I>(
        &self,
        resource: &R,
        fill_rule: FillRule,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey;

    /// Slices the current shapes by string lines.
    ///
    /// - `resource`: A string lines.
    ///   `ShapeResource` can be one of the following:
    ///     - `Path`: A path representing a string line.
    ///     - `Paths`: A collection of paths, each representing a string line.
    ///     - `Vec<Paths>`: A collection of grouped paths, where each group may consist of multiple paths.
    /// - `fill_rule`: Fill rule to determine filled areas (non-zero, even-odd, positive, negative).
    /// - `options`: Adjust custom behavior.
    /// - `solver`: Type of solver to use.
    /// - Returns a `Shapes<P>` collection representing the sliced geometry.
    ///
    /// Note: Outer boundary paths have a **main_direction** order, and holes have an opposite to **main_direction** order.
    fn slice_custom_by(
        &self,
        resource: &R,
        fill_rule: FillRule,
        options: OverlayOptions<P::Scalar>,
        solver: Solver,
    ) -> Shapes<P>;

    /// Same as [`Self::slice_custom_by`], but with an explicit integer engine.
    fn slice_custom_by_as<I>(
        &self,
        resource: &R,
        fill_rule: FillRule,
        options: OverlayOptions<P::Scalar, I>,
        solver: Solver,
    ) -> Shapes<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey;

    /// Slices the current shapes by string lines with a fixed float-to-integer scale.
    ///
    /// - `resource`: A string lines.
    ///   `ShapeResource` can be one of the following:
    ///     - `Path`: A path representing a string line.
    ///     - `Paths`: A collection of paths, each representing a string line.
    ///     - `Vec<Paths>`: A collection of grouped paths, where each group may consist of multiple paths.
    /// - `fill_rule`: Fill rule to determine filled areas (non-zero, even-odd, positive, negative).
    /// - `options`: Adjust custom behavior.
    /// - `solver`: Type of solver to use.
    /// - `scale`: Fixed float-to-integer scale. Use `scale = 1.0 / grid_size` if you prefer grid size semantics.
    /// - Returns a `Shapes<P>` collection representing the sliced geometry.
    ///
    /// Note: Outer boundary paths have a **main_direction** order, and holes have an opposite to **main_direction** order.
    fn slice_custom_by_fixed_scale(
        &self,
        resource: &R,
        fill_rule: FillRule,
        options: OverlayOptions<P::Scalar>,
        solver: Solver,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError>;

    /// Same as [`Self::slice_custom_by_fixed_scale`], but with an explicit integer engine.
    fn slice_custom_by_fixed_scale_as<I>(
        &self,
        resource: &R,
        fill_rule: FillRule,
        options: OverlayOptions<P::Scalar, I>,
        solver: Solver,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey;
}

impl<R0, R1, P> FloatSlice<R0, P> for R1
where
    R0: ShapeResource<P>,
    R1: ShapeResource<P>,
    P: FloatPointCompatible,
{
    #[inline]
    fn slice_by(&self, resource: &R0, fill_rule: FillRule) -> Shapes<P> {
        FloatStringOverlay::<P>::with_shape_and_string(self, resource)
            .build_graph_view(fill_rule)
            .map(|graph| graph.extract_shapes(StringRule::Slice))
            .unwrap_or_default()
    }

    #[inline]
    fn slice_by_as<I>(&self, resource: &R0, fill_rule: FillRule) -> Shapes<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey,
    {
        FloatStringOverlay::<P, I>::from_shape_and_string(self, resource)
            .build_graph_view(fill_rule)
            .map(|graph| graph.extract_shapes(StringRule::Slice))
            .unwrap_or_default()
    }

    #[inline]
    fn slice_by_fixed_scale(
        &self,
        resource: &R0,
        fill_rule: FillRule,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError> {
        Ok(
            FloatStringOverlay::<P>::with_shape_and_string_fixed_scale(self, resource, scale)?
                .build_graph_view(fill_rule)
                .map(|graph| graph.extract_shapes(StringRule::Slice))
                .unwrap_or_default(),
        )
    }

    #[inline]
    fn slice_by_fixed_scale_as<I>(
        &self,
        resource: &R0,
        fill_rule: FillRule,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey,
    {
        Ok(
            FloatStringOverlay::<P, I>::from_shape_and_string_fixed_scale(self, resource, scale)?
                .build_graph_view(fill_rule)
                .map(|graph| graph.extract_shapes(StringRule::Slice))
                .unwrap_or_default(),
        )
    }

    #[inline]
    fn slice_custom_by(
        &self,
        resource: &R0,
        fill_rule: FillRule,
        options: OverlayOptions<P::Scalar>,
        solver: Solver,
    ) -> Shapes<P> {
        FloatStringOverlay::<P>::with_shape_and_string(self, resource)
            .build_graph_view_with_solver(fill_rule, solver)
            .map(|graph| graph.extract_shapes_custom(StringRule::Slice, options))
            .unwrap_or_default()
    }

    #[inline]
    fn slice_custom_by_as<I>(
        &self,
        resource: &R0,
        fill_rule: FillRule,
        options: OverlayOptions<P::Scalar, I>,
        solver: Solver,
    ) -> Shapes<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey,
    {
        FloatStringOverlay::<P, I>::from_shape_and_string(self, resource)
            .build_graph_view_with_solver(fill_rule, solver)
            .map(|graph| graph.extract_shapes_custom(StringRule::Slice, options))
            .unwrap_or_default()
    }

    #[inline]
    fn slice_custom_by_fixed_scale(
        &self,
        resource: &R0,
        fill_rule: FillRule,
        options: OverlayOptions<P::Scalar>,
        solver: Solver,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError> {
        Ok(
            FloatStringOverlay::<P>::with_shape_and_string_fixed_scale(self, resource, scale)?
                .build_graph_view_with_solver(fill_rule, solver)
                .map(|graph| graph.extract_shapes_custom(StringRule::Slice, options))
                .unwrap_or_default(),
        )
    }

    #[inline]
    fn slice_custom_by_fixed_scale_as<I>(
        &self,
        resource: &R0,
        fill_rule: FillRule,
        options: OverlayOptions<P::Scalar, I>,
        solver: Solver,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey,
    {
        Ok(
            FloatStringOverlay::<P, I>::from_shape_and_string_fixed_scale(self, resource, scale)?
                .build_graph_view_with_solver(fill_rule, solver)
                .map(|graph| graph.extract_shapes_custom(StringRule::Slice, options))
                .unwrap_or_default(),
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::core::fill_rule::FillRule;
    use crate::float::simplify::SimplifyShape;
    use crate::float::slice::FloatSlice;
    use alloc::vec;

    #[test]
    fn test_contour_slice() {
        let rect = [[0.0, 0.0], [0.0, 0.5], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];

        let shapes = rect.as_slice().simplify_shape(FillRule::NonZero);

        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].len(), 1);
        assert_eq!(shapes[0][0].len(), 4);
    }

    #[test]
    fn test_contour_vec() {
        let rect = vec![[0.0, 0.0], [0.0, 0.5], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];

        let shapes = rect.simplify_shape(FillRule::NonZero);

        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].len(), 1);
        assert_eq!(shapes[0][0].len(), 4);
    }

    #[test]
    fn test_slice_fixed_scale_ok() {
        let shape = vec![[0.0, 0.0], [0.0, 2.0], [2.0, 2.0], [2.0, 0.0]];
        let string = vec![[-1.0, 1.0], [3.0, 1.0]];

        let shapes = shape
            .slice_by_fixed_scale(&string, FillRule::EvenOdd, 10.0)
            .unwrap();

        assert_eq!(shapes.len(), 2);
        assert_eq!(shapes[0].len(), 1);
        assert_eq!(shapes[1].len(), 1);
        assert_eq!(shapes[0][0].len(), 4);
        assert_eq!(shapes[1][0].len(), 4);

        let shapes = shape
            .slice_by_fixed_scale_as::<i64>(&string, FillRule::EvenOdd, 10.0)
            .unwrap();

        assert_eq!(shapes.len(), 2);
    }

    #[test]
    fn test_slice_fixed_scale_invalid() {
        let shape = vec![[0.0, 0.0], [0.0, 2.0], [2.0, 2.0], [2.0, 0.0]];
        let string = vec![[-1.0, 1.0], [3.0, 1.0]];

        assert!(
            shape
                .slice_by_fixed_scale(&string, FillRule::EvenOdd, 0.0)
                .is_err()
        );
        assert!(
            shape
                .slice_by_fixed_scale(&string, FillRule::EvenOdd, -1.0)
                .is_err()
        );
        assert!(
            shape
                .slice_by_fixed_scale(&string, FillRule::EvenOdd, f64::NAN)
                .is_err()
        );
        assert!(
            shape
                .slice_by_fixed_scale(&string, FillRule::EvenOdd, f64::INFINITY)
                .is_err()
        );
    }
}
