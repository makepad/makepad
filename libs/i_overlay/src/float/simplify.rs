use crate::core::fill_rule::FillRule;
use crate::core::overlay_rule::OverlayRule;
use crate::core::solver::Solver;
use crate::float::overlay::{FloatOverlay, OverlayOptions};
use i_float::float::compatible::FloatPointCompatible;
use i_float::int::number::int::IntNumber;
use i_key_sort::sort::key::SortKey;
use i_shape::base::data::Shapes;
use i_shape::source::resource::ShapeResource;
use i_tree::{Expiration, LayoutNumber};

/// Trait `Simplify` provides a method to simplify geometric shapes by reducing the number of points in contours or shapes
/// while preserving overall shape and topology. The method applies a minimum area threshold and a build rule to
/// determine which areas should be retained or excluded.
///
/// This convenience trait uses the default integer engine (`i32`). Use the `*_as::<I>` methods
/// when you need to select `i16`, `i32`, or `i64` explicitly.
///
/// # Example
///
/// ```
/// use i_overlay::core::fill_rule::FillRule;
/// use i_overlay::float::simplify::SimplifyShape;
///
/// let shape = vec![[0.0, 0.0], [0.0, 0.5], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];
///
/// let result = shape.simplify_shape_as::<i64>(FillRule::NonZero);
///
/// assert_eq!(result.len(), 1);
/// ```
pub trait SimplifyShape<P: FloatPointCompatible> {
    /// Simplifies the shape or collection of points, contours, or shapes, based on a specified minimum area threshold.
    ///
    /// - Returns: A collection of `Shapes<P>` that represents the simplified geometry.
    ///
    /// Note: Outer boundary paths have a **main_direction** order, and holes have an opposite to **main_direction** order.
    fn simplify_shape(&self, fill_rule: FillRule) -> Shapes<P>;

    /// Same as [`Self::simplify_shape`], but with an explicit integer engine.
    fn simplify_shape_as<I>(&self, fill_rule: FillRule) -> Shapes<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey;

    /// Simplifies the shape or collection of points, contours, or shapes, based on a specified minimum area threshold.
    /// - `options`: Adjust custom behavior.
    /// - `solver`: Type of solver to use.
    /// - Returns: A collection of Shapes<P> that represents the simplified geometry.
    ///
    /// Note: Outer boundary paths have a **main_direction** order, and holes have an opposite to **main_direction** order.
    fn simplify_shape_custom(
        &self,
        fill_rule: FillRule,
        options: OverlayOptions<P::Scalar>,
        solver: Solver,
    ) -> Shapes<P>;

    /// Same as [`Self::simplify_shape_custom`], but with an explicit integer engine.
    fn simplify_shape_custom_as<I>(
        &self,
        fill_rule: FillRule,
        options: OverlayOptions<P::Scalar, I>,
        solver: Solver,
    ) -> Shapes<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey;
}

impl<S, P> SimplifyShape<P> for S
where
    S: ShapeResource<P>,
    P: FloatPointCompatible,
{
    #[inline]
    fn simplify_shape(&self, fill_rule: FillRule) -> Shapes<P> {
        FloatOverlay::<P>::with_subj_custom(self, Default::default(), Default::default())
            .overlay(OverlayRule::Subject, fill_rule)
    }

    #[inline]
    fn simplify_shape_as<I>(&self, fill_rule: FillRule) -> Shapes<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey,
    {
        FloatOverlay::<P, I>::from_subj_custom(self, Default::default(), Default::default())
            .overlay(OverlayRule::Subject, fill_rule)
    }

    #[inline]
    fn simplify_shape_custom(
        &self,
        fill_rule: FillRule,
        options: OverlayOptions<P::Scalar>,
        solver: Solver,
    ) -> Shapes<P> {
        FloatOverlay::<P>::with_subj_custom(self, options, solver).overlay(OverlayRule::Subject, fill_rule)
    }

    #[inline]
    fn simplify_shape_custom_as<I>(
        &self,
        fill_rule: FillRule,
        options: OverlayOptions<P::Scalar, I>,
        solver: Solver,
    ) -> Shapes<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey,
    {
        FloatOverlay::<P, I>::from_subj_custom(self, options, solver).overlay(OverlayRule::Subject, fill_rule)
    }
}

#[cfg(test)]
mod tests {
    use crate::core::fill_rule::FillRule;
    use crate::float::simplify::SimplifyShape;
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
    fn test_contour_slice_as() {
        let rect = [[0.0, 0.0], [0.0, 0.5], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];

        let shapes = rect.as_slice().simplify_shape_as::<i64>(FillRule::NonZero);

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
}
