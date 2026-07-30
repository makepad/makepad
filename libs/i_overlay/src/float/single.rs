use crate::core::fill_rule::FillRule;
use crate::core::overlay_rule::OverlayRule;
use crate::float::overlay::FloatOverlay;
use i_float::float::compatible::FloatPointCompatible;
use i_float::int::number::int::IntNumber;
use i_key_sort::sort::key::SortKey;
use i_shape::base::data::Shapes;
use i_shape::source::resource::ShapeResource;
use i_tree::{Expiration, LayoutNumber};

/// Trait `SingleFloatOverlay` provides methods for overlay operations between various geometric entities.
/// This trait supports boolean operations on contours, shapes, and collections of shapes, using customizable overlay and build rules.
///
/// This convenience trait uses the default integer engine (`i32`). Use the `*_as::<I>` methods
/// when you need to select `i16`, `i32`, or `i64` explicitly.
///
/// # Example
///
/// ```
/// use i_overlay::core::fill_rule::FillRule;
/// use i_overlay::core::overlay_rule::OverlayRule;
/// use i_overlay::float::single::SingleFloatOverlay;
///
/// let subj = vec![[0.0, 0.0], [0.0, 5.0], [5.0, 5.0], [5.0, 0.0]];
/// let clip = vec![[2.0, 2.0], [2.0, 4.0], [4.0, 4.0], [4.0, 2.0]];
///
/// let result = subj.overlay_as::<i64>(&clip, OverlayRule::Difference, FillRule::EvenOdd);
///
/// assert_eq!(result.len(), 1);
/// ```
pub trait SingleFloatOverlay<R0, R1, P>
where
    R0: ShapeResource<P>,
    R1: ShapeResource<P>,
    P: FloatPointCompatible,
{
    /// General overlay method that takes an `ShapeResource` to determine the input type.
    ///
    /// - `resource`: A `ShapeResource` specifying the type of geometric entity to overlay with.
    ///   It can be one of the following:
    ///     - `Contour`: A single contour representing a path or boundary.
    ///     - `Contours`: A collection of contours, each defining separate boundaries.
    ///     - `Shapes`: A collection of shapes, where each shape may consist of multiple contours.
    /// - `overlay_rule`: The boolean operation rule to apply when extracting shapes from the graph, such as union or intersection.
    /// - `fill_rule`: Fill rule to determine filled areas (non-zero, even-odd, positive, negative).
    /// - Returns: A vector of `Shapes<P>` representing the cleaned-up geometric result.
    fn overlay(&self, source: &R1, overlay_rule: OverlayRule, fill_rule: FillRule) -> Shapes<P>;

    /// Same as [`Self::overlay`], but with an explicit integer engine.
    fn overlay_as<I>(&self, source: &R1, overlay_rule: OverlayRule, fill_rule: FillRule) -> Shapes<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey;
}

impl<R0, R1, P> SingleFloatOverlay<R0, R1, P> for R0
where
    R0: ShapeResource<P>,
    R1: ShapeResource<P>,
    P: FloatPointCompatible,
{
    #[inline]
    fn overlay(&self, resource: &R1, overlay_rule: OverlayRule, fill_rule: FillRule) -> Shapes<P> {
        FloatOverlay::<P>::with_subj_and_clip(self, resource).overlay(overlay_rule, fill_rule)
    }

    #[inline]
    fn overlay_as<I>(&self, resource: &R1, overlay_rule: OverlayRule, fill_rule: FillRule) -> Shapes<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey,
    {
        FloatOverlay::<P, I>::from_subj_and_clip(self, resource).overlay(overlay_rule, fill_rule)
    }
}

#[cfg(test)]
mod tests {
    use crate::core::fill_rule::FillRule;
    use crate::core::overlay_rule::OverlayRule;
    use crate::float::overlay::FloatOverlay;
    use crate::float::single::SingleFloatOverlay;
    use alloc::vec;

    #[test]
    fn test_contour() {
        let left_rect = vec![[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];
        let right_rect = vec![[1.0, 0.0], [1.0, 1.0], [2.0, 1.0], [2.0, 0.0]];

        let shapes = left_rect.overlay(&right_rect, OverlayRule::Union, FillRule::EvenOdd);

        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].len(), 1);
        assert_eq!(shapes[0][0].len(), 4);
    }

    #[test]
    fn test_contour_as() {
        let left_rect = vec![[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];
        let right_rect = vec![[1.0, 0.0], [1.0, 1.0], [2.0, 1.0], [2.0, 0.0]];

        let shapes = left_rect.overlay_as::<i64>(&right_rect, OverlayRule::Union, FillRule::EvenOdd);

        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].len(), 1);
        assert_eq!(shapes[0][0].len(), 4);
    }

    #[test]
    fn test_contours() {
        let r3 = vec![
            vec![[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]],
            vec![[0.0, 1.0], [0.0, 2.0], [1.0, 2.0], [1.0, 1.0]],
            vec![[1.0, 1.0], [1.0, 2.0], [2.0, 2.0], [2.0, 1.0]],
        ];
        let right_bottom_rect = vec![[1.0, 0.0], [1.0, 1.0], [2.0, 1.0], [2.0, 0.0]];

        let shapes = FloatOverlay::with_subj_and_clip(&r3, &right_bottom_rect)
            .overlay(OverlayRule::Union, FillRule::EvenOdd);

        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].len(), 1);
        assert_eq!(shapes[0][0].len(), 4);
    }

    #[test]
    fn test_shapes() {
        let shapes = vec![vec![
            vec![[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]],
            vec![[0.0, 1.0], [0.0, 2.0], [1.0, 2.0], [1.0, 1.0]],
            vec![[1.0, 1.0], [1.0, 2.0], [2.0, 2.0], [2.0, 1.0]],
        ]];
        let right_bottom_rect = vec![[1.0, 0.0], [1.0, 1.0], [2.0, 1.0], [2.0, 0.0]];

        let shapes = FloatOverlay::with_subj_and_clip(&shapes, &right_bottom_rect)
            .overlay(OverlayRule::Union, FillRule::EvenOdd);

        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].len(), 1);
        assert_eq!(shapes[0][0].len(), 4);
    }
}
