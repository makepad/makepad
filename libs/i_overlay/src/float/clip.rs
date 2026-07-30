use crate::core::fill_rule::FillRule;
use crate::core::solver::Solver;
use crate::float::scale::FixedScaleOverlayError;
use crate::float::string_overlay::FloatStringOverlay;
use crate::string::clip::ClipRule;
use i_float::float::compatible::FloatPointCompatible;
use i_float::int::number::int::IntNumber;
use i_key_sort::sort::key::SortKey;
use i_shape::base::data::Paths;
use i_shape::source::resource::ShapeResource;
use i_tree::{Expiration, LayoutNumber};

/// Trait for clipping float string paths by float shapes.
///
/// This convenience trait uses the default integer engine (`i32`). Use the `*_as::<I>` methods
/// when you need to select `i16`, `i32`, or `i64` explicitly.
///
/// # Example
///
/// ```
/// use i_overlay::core::fill_rule::FillRule;
/// use i_overlay::float::clip::FloatClip;
/// use i_overlay::string::clip::ClipRule;
///
/// let shape = vec![[0.0, 0.0], [0.0, 2.0], [2.0, 2.0], [2.0, 0.0]];
/// let line = vec![[-1.0, 1.0], [3.0, 1.0]];
/// let clip_rule = ClipRule { invert: false, boundary_included: false };
///
/// let result = line.clip_by_as::<i64>(&shape, FillRule::EvenOdd, clip_rule);
///
/// assert_eq!(result.len(), 1);
/// ```
pub trait FloatClip<R, P>
where
    R: ShapeResource<P>,
    P: FloatPointCompatible,
{
    /// Clips paths according to the specified build and clip rules.
    /// - `resource`: A clipping shape.
    ///   `ShapeResource` can be one of the following:
    ///     - `Contour`: A contour representing a closed path. This path is interpreted as closed, so it doesn’t require the start and endpoint to be the same for processing.
    ///     - `Contours`: A collection of contours, each representing a closed path.
    ///     - `Shapes`: A collection of shapes, where each shape may consist of multiple contours.
    /// - `fill_rule`: Fill rule to determine filled areas (non-zero, even-odd, positive, negative).
    /// - `clip_rule`: Clip rule to determine how boundary and inversion settings affect the result.
    ///
    /// # Returns
    /// A `Paths<P>` collection of string lines that meet the clipping conditions.
    fn clip_by(&self, source: &R, fill_rule: FillRule, clip_rule: ClipRule) -> Paths<P>;

    /// Same as [`Self::clip_by`], but with an explicit integer engine.
    fn clip_by_as<I>(&self, source: &R, fill_rule: FillRule, clip_rule: ClipRule) -> Paths<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey;

    /// Clips paths according to the specified build and clip rules using a fixed float-to-integer scale.
    /// - `resource`: A clipping shape.
    ///   `ShapeResource` can be one of the following:
    ///     - `Contour`: A contour representing a closed path. This path is interpreted as closed, so it doesn’t require the start and endpoint to be the same for processing.
    ///     - `Contours`: A collection of contours, each representing a closed path.
    ///     - `Shapes`: A collection of shapes, where each shape may consist of multiple contours.
    /// - `fill_rule`: Fill rule to determine filled areas (non-zero, even-odd, positive, negative).
    /// - `clip_rule`: Clip rule to determine how boundary and inversion settings affect the result.
    /// - `scale`: Fixed float-to-integer scale. Use `scale = 1.0 / grid_size` if you prefer grid size semantics.
    ///
    /// # Returns
    /// A `Paths<P>` collection of string lines that meet the clipping conditions.
    fn clip_by_fixed_scale(
        &self,
        source: &R,
        fill_rule: FillRule,
        clip_rule: ClipRule,
        scale: P::Scalar,
    ) -> Result<Paths<P>, FixedScaleOverlayError>;

    /// Same as [`Self::clip_by_fixed_scale`], but with an explicit integer engine.
    fn clip_by_fixed_scale_as<I>(
        &self,
        source: &R,
        fill_rule: FillRule,
        clip_rule: ClipRule,
        scale: P::Scalar,
    ) -> Result<Paths<P>, FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey;

    /// Clips paths according to the specified build and clip rules.
    /// - `resource`: A clipping shape.
    ///   `ShapeResource` can be one of the following:
    ///     - `Contour`: A contour representing a closed path. This path is interpreted as closed, so it doesn’t require the start and endpoint to be the same for processing.
    ///     - `Contours`: A collection of contours, each representing a closed path.
    ///     - `Shapes`: A collection of shapes, where each shape may consist of multiple contours.
    /// - `fill_rule`: Fill rule to determine filled areas (non-zero, even-odd, positive, negative).
    /// - `clip_rule`: Clip rule to determine how boundary and inversion settings affect the result.
    /// - `solver`: Type of solver to use.
    ///
    /// # Returns
    /// A `Paths<P>` collection of string lines that meet the clipping conditions.
    fn clip_by_with_solver(
        &self,
        source: &R,
        fill_rule: FillRule,
        clip_rule: ClipRule,
        solver: Solver,
    ) -> Paths<P>;

    /// Same as [`Self::clip_by_with_solver`], but with an explicit integer engine.
    fn clip_by_with_solver_as<I>(
        &self,
        source: &R,
        fill_rule: FillRule,
        clip_rule: ClipRule,
        solver: Solver,
    ) -> Paths<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey;

    /// Clips paths according to the specified build and clip rules using a fixed float-to-integer scale.
    /// - `resource`: A clipping shape.
    ///   `ShapeResource` can be one of the following:
    ///     - `Contour`: A contour representing a closed path. This path is interpreted as closed, so it doesn’t require the start and endpoint to be the same for processing.
    ///     - `Contours`: A collection of contours, each representing a closed path.
    ///     - `Shapes`: A collection of shapes, where each shape may consist of multiple contours.
    /// - `fill_rule`: Fill rule to determine filled areas (non-zero, even-odd, positive, negative).
    /// - `clip_rule`: Clip rule to determine how boundary and inversion settings affect the result.
    /// - `solver`: Type of solver to use.
    /// - `scale`: Fixed float-to-integer scale. Use `scale = 1.0 / grid_size` if you prefer grid size semantics.
    ///
    /// # Returns
    /// A `Paths<P>` collection of string lines that meet the clipping conditions.
    fn clip_by_fixed_scale_with_solver(
        &self,
        source: &R,
        fill_rule: FillRule,
        clip_rule: ClipRule,
        solver: Solver,
        scale: P::Scalar,
    ) -> Result<Paths<P>, FixedScaleOverlayError>;

    /// Same as [`Self::clip_by_fixed_scale_with_solver`], but with an explicit integer engine.
    fn clip_by_fixed_scale_with_solver_as<I>(
        &self,
        source: &R,
        fill_rule: FillRule,
        clip_rule: ClipRule,
        solver: Solver,
        scale: P::Scalar,
    ) -> Result<Paths<P>, FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey;
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use crate::core::fill_rule::FillRule;
    use crate::float::clip::FloatClip;
    use crate::string::clip::ClipRule;
    use alloc::vec;

    #[test]
    fn test_clip_fixed_scale_ok() {
        let shape = vec![[0.0, 0.0], [0.0, 2.0], [2.0, 2.0], [2.0, 0.0]];
        let string = vec![[-1.0, 1.0], [3.0, 1.0]];

        let clip_rule = ClipRule {
            invert: false,
            boundary_included: false,
        };
        let paths = string
            .clip_by_fixed_scale(&shape, FillRule::EvenOdd, clip_rule, 10.0)
            .unwrap();

        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], [[0.0, 1.0], [2.0, 1.0]]);

        let paths = string
            .clip_by_fixed_scale_as::<i64>(&shape, FillRule::EvenOdd, clip_rule, 10.0)
            .unwrap();

        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn test_clip_fixed_scale_invalid() {
        let shape = vec![[0.0, 0.0], [0.0, 2.0], [2.0, 2.0], [2.0, 0.0]];
        let string = vec![[-1.0, 1.0], [3.0, 1.0]];

        let clip_rule = ClipRule {
            invert: false,
            boundary_included: false,
        };
        assert!(
            string
                .clip_by_fixed_scale(&shape, FillRule::EvenOdd, clip_rule, 0.0)
                .is_err()
        );
        assert!(
            string
                .clip_by_fixed_scale(&shape, FillRule::EvenOdd, clip_rule, -1.0)
                .is_err()
        );
        assert!(
            string
                .clip_by_fixed_scale(&shape, FillRule::EvenOdd, clip_rule, f64::NAN)
                .is_err()
        );
        assert!(
            string
                .clip_by_fixed_scale(&shape, FillRule::EvenOdd, clip_rule, f64::INFINITY)
                .is_err()
        );
    }
}

impl<R0, R1, P> FloatClip<R0, P> for R1
where
    R0: ShapeResource<P>,
    R1: ShapeResource<P>,
    P: FloatPointCompatible,
{
    #[inline]
    fn clip_by(&self, resource: &R0, fill_rule: FillRule, clip_rule: ClipRule) -> Paths<P> {
        self.clip_by_with_solver(resource, fill_rule, clip_rule, Default::default())
    }

    #[inline]
    fn clip_by_as<I>(&self, resource: &R0, fill_rule: FillRule, clip_rule: ClipRule) -> Paths<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey,
    {
        self.clip_by_with_solver_as::<I>(resource, fill_rule, clip_rule, Default::default())
    }

    #[inline]
    fn clip_by_with_solver(
        &self,
        resource: &R0,
        fill_rule: FillRule,
        clip_rule: ClipRule,
        solver: Solver,
    ) -> Paths<P> {
        FloatStringOverlay::<P>::with_shape_and_string(resource, self)
            .clip_string_lines_with_solver(fill_rule, clip_rule, solver)
    }

    #[inline]
    fn clip_by_with_solver_as<I>(
        &self,
        resource: &R0,
        fill_rule: FillRule,
        clip_rule: ClipRule,
        solver: Solver,
    ) -> Paths<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey,
    {
        FloatStringOverlay::<P, I>::from_shape_and_string(resource, self)
            .clip_string_lines_with_solver(fill_rule, clip_rule, solver)
    }

    #[inline]
    fn clip_by_fixed_scale(
        &self,
        resource: &R0,
        fill_rule: FillRule,
        clip_rule: ClipRule,
        scale: P::Scalar,
    ) -> Result<Paths<P>, FixedScaleOverlayError> {
        self.clip_by_fixed_scale_with_solver(resource, fill_rule, clip_rule, Default::default(), scale)
    }

    #[inline]
    fn clip_by_fixed_scale_as<I>(
        &self,
        resource: &R0,
        fill_rule: FillRule,
        clip_rule: ClipRule,
        scale: P::Scalar,
    ) -> Result<Paths<P>, FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey,
    {
        self.clip_by_fixed_scale_with_solver_as::<I>(
            resource,
            fill_rule,
            clip_rule,
            Default::default(),
            scale,
        )
    }

    #[inline]
    fn clip_by_fixed_scale_with_solver(
        &self,
        resource: &R0,
        fill_rule: FillRule,
        clip_rule: ClipRule,
        solver: Solver,
        scale: P::Scalar,
    ) -> Result<Paths<P>, FixedScaleOverlayError> {
        Ok(
            FloatStringOverlay::<P>::with_shape_and_string_fixed_scale(resource, self, scale)?
                .clip_string_lines_with_solver(fill_rule, clip_rule, solver),
        )
    }

    #[inline]
    fn clip_by_fixed_scale_with_solver_as<I>(
        &self,
        resource: &R0,
        fill_rule: FillRule,
        clip_rule: ClipRule,
        solver: Solver,
        scale: P::Scalar,
    ) -> Result<Paths<P>, FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey,
    {
        Ok(
            FloatStringOverlay::<P, I>::from_shape_and_string_fixed_scale(resource, self, scale)?
                .clip_string_lines_with_solver(fill_rule, clip_rule, solver),
        )
    }
}
