use crate::core::extract::BooleanExtractionBuffer;
use crate::core::fill_rule::FillRule;
use crate::core::overlay::ShapeType::Subject;
use crate::core::overlay::{ContourDirection, Overlay};
use crate::core::overlay_rule::OverlayRule;
use crate::float::overlay::OverlayOptions;
use crate::float::scale::FixedScaleOverlayError;
use crate::mesh::outline::builder::OutlineBuilder;
use crate::mesh::style::OutlineStyle;
use alloc::vec;
use alloc::vec::Vec;
use i_float::adapter::FloatPointAdapter;
use i_float::float::compatible::FloatPointCompatible;
use i_float::float::number::FloatNumber;
use i_float::float::rect::FloatRect;
use i_float::int::number::int::IntNumber;
use i_float::int::number::uint::UIntNumber;
use i_float::int::number::wide_int::WideIntNumber;
use i_key_sort::sort::key::SortKey;
use i_shape::base::data::Shapes;
use i_shape::flat::buffer::FlatContoursBuffer;
use i_shape::flat::float::FloatFlatContoursBuffer;
use i_shape::float::adapter::ShapesToFloat;
use i_shape::float::despike::DeSpikeContour;
use i_shape::float::int_area::IntArea;
use i_shape::float::simple::SimplifyContour;
use i_shape::source::resource::ShapeResource;
use i_tree::{Expiration, LayoutNumber};

/// Trait for offsetting float contours and shapes.
///
/// Default methods use the `i32` integer engine. Use the `*_as::<I>` methods when you need to
/// select `i16`, `i32`, or `i64` explicitly.
///
/// # Example
///
/// ```
/// use i_overlay::mesh::outline::offset::OutlineOffset;
/// use i_overlay::mesh::style::OutlineStyle;
///
/// let path = [[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]];
/// let style = OutlineStyle::new(1.0);
///
/// let result = path.outline_as::<i64>(&style);
///
/// assert_eq!(result.len(), 1);
/// ```
pub trait OutlineOffset<P: FloatPointCompatible> {
    /// Generates an outline shapes for contours, or shapes.
    ///
    /// - `style`: Defines the outline properties, including offset, and joins.
    ///
    /// # Returns
    /// A collection of `Shapes<P>` representing the outline geometry.
    /// Note: Outer boundary paths have a counterclockwise order, and holes have a clockwise order.
    fn outline(&self, style: &OutlineStyle<P::Scalar>) -> Shapes<P>;

    /// Generates outline contours directly into a flat buffer.
    ///
    /// - `style`: Defines the outline properties, including offset, and joins.
    /// - `output`: Destination buffer that receives resulting contours. Existing contents are replaced.
    fn outline_into(&self, style: &OutlineStyle<P::Scalar>, output: &mut FloatFlatContoursBuffer<P>);

    /// Generates an outline shapes for contours, or shapes with optional filtering.
    ///
    /// - `style`: Defines the outline properties, including offset, and joins.
    /// - `options`: Adjust custom behavior.
    ///
    /// # Returns
    /// A collection of `Shapes<P>` representing the outline geometry.
    /// Note: Outer boundary paths have a **main_direction** order, and holes have an opposite to **main_direction** order.
    fn outline_custom(
        &self,
        style: &OutlineStyle<P::Scalar>,
        options: OverlayOptions<P::Scalar>,
    ) -> Shapes<P>;

    /// Generates outline contours directly into a flat buffer with optional filtering.
    ///
    /// - `style`: Defines the outline properties, including offset, and joins.
    /// - `options`: Adjust custom behavior.
    /// - `output`: Destination buffer that receives resulting contours. Existing contents are replaced.
    fn outline_custom_into(
        &self,
        style: &OutlineStyle<P::Scalar>,
        options: OverlayOptions<P::Scalar>,
        output: &mut FloatFlatContoursBuffer<P>,
    );

    /// Generates an outline shapes for contours, or shapes with a fixed float-to-integer scale.
    ///
    /// - `style`: Defines the outline properties, including offset, and joins.
    /// - `scale`: Fixed float-to-integer scale. Use `scale = 1.0 / grid_size` if you prefer grid size semantics.
    ///
    /// # Returns
    /// A collection of `Shapes<P>` representing the outline geometry.
    /// Note: Outer boundary paths have a counterclockwise order, and holes have a clockwise order.
    fn outline_fixed_scale(
        &self,
        style: &OutlineStyle<P::Scalar>,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError>;

    /// Generates outline contours directly into a flat buffer with fixed float-to-integer scale.
    ///
    /// - `style`: Defines the outline properties, including offset, and joins.
    /// - `scale`: Fixed float-to-integer scale. Use `scale = 1.0 / grid_size` if you prefer grid size semantics.
    /// - `output`: Destination buffer that receives resulting contours. Existing contents are replaced on success.
    fn outline_fixed_scale_into(
        &self,
        style: &OutlineStyle<P::Scalar>,
        scale: P::Scalar,
        output: &mut FloatFlatContoursBuffer<P>,
    ) -> Result<(), FixedScaleOverlayError>;

    /// Generates an outline shapes for contours, or shapes with optional filtering and fixed scaling.
    ///
    /// - `style`: Defines the outline properties, including offset, and joins.
    /// - `options`: Adjust custom behavior.
    /// - `scale`: Fixed float-to-integer scale. Use `scale = 1.0 / grid_size` if you prefer grid size semantics.
    ///
    /// # Returns
    /// A collection of `Shapes<P>` representing the outline geometry.
    /// Note: Outer boundary paths have a **main_direction** order, and holes have an opposite to **main_direction** order.
    fn outline_custom_fixed_scale(
        &self,
        style: &OutlineStyle<P::Scalar>,
        options: OverlayOptions<P::Scalar>,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError>;

    /// Generates outline contours directly into a flat buffer with optional filtering and fixed scaling.
    ///
    /// - `style`: Defines the outline properties, including offset, and joins.
    /// - `options`: Adjust custom behavior.
    /// - `scale`: Fixed float-to-integer scale. Use `scale = 1.0 / grid_size` if you prefer grid size semantics.
    /// - `output`: Destination buffer that receives resulting contours. Existing contents are replaced on success.
    fn outline_custom_fixed_scale_into(
        &self,
        style: &OutlineStyle<P::Scalar>,
        options: OverlayOptions<P::Scalar>,
        scale: P::Scalar,
        output: &mut FloatFlatContoursBuffer<P>,
    ) -> Result<(), FixedScaleOverlayError>;

    /// Same as [`Self::outline`], but with an explicit integer engine.
    fn outline_as<I>(&self, style: &OutlineStyle<P::Scalar>) -> Shapes<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static;

    /// Same as [`Self::outline_into`], but with an explicit integer engine.
    fn outline_into_as<I>(&self, style: &OutlineStyle<P::Scalar>, output: &mut FloatFlatContoursBuffer<P>)
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static;

    /// Same as [`Self::outline_custom`], but with an explicit integer engine.
    fn outline_custom_as<I>(
        &self,
        style: &OutlineStyle<P::Scalar>,
        options: OverlayOptions<P::Scalar, I>,
    ) -> Shapes<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static;

    /// Same as [`Self::outline_custom_into`], but with an explicit integer engine.
    fn outline_custom_into_as<I>(
        &self,
        style: &OutlineStyle<P::Scalar>,
        options: OverlayOptions<P::Scalar, I>,
        output: &mut FloatFlatContoursBuffer<P>,
    ) where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static;

    /// Same as [`Self::outline_fixed_scale`], but with an explicit integer engine.
    fn outline_fixed_scale_as<I>(
        &self,
        style: &OutlineStyle<P::Scalar>,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static;

    /// Same as [`Self::outline_fixed_scale_into`], but with an explicit integer engine.
    fn outline_fixed_scale_into_as<I>(
        &self,
        style: &OutlineStyle<P::Scalar>,
        scale: P::Scalar,
        output: &mut FloatFlatContoursBuffer<P>,
    ) -> Result<(), FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static;

    /// Same as [`Self::outline_custom_fixed_scale`], but with an explicit integer engine.
    fn outline_custom_fixed_scale_as<I>(
        &self,
        style: &OutlineStyle<P::Scalar>,
        options: OverlayOptions<P::Scalar, I>,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static;

    /// Same as [`Self::outline_custom_fixed_scale_into`], but with an explicit integer engine.
    fn outline_custom_fixed_scale_into_as<I>(
        &self,
        style: &OutlineStyle<P::Scalar>,
        options: OverlayOptions<P::Scalar, I>,
        scale: P::Scalar,
        output: &mut FloatFlatContoursBuffer<P>,
    ) -> Result<(), FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static;
}

impl<S, P> OutlineOffset<P> for S
where
    S: ShapeResource<P>,
    P: FloatPointCompatible + 'static,
{
    fn outline(&self, style: &OutlineStyle<P::Scalar>) -> Shapes<P> {
        self.outline_custom(style, Default::default())
    }

    fn outline_into(&self, style: &OutlineStyle<P::Scalar>, output: &mut FloatFlatContoursBuffer<P>) {
        self.outline_custom_into(style, Default::default(), output)
    }

    fn outline_custom(
        &self,
        style: &OutlineStyle<P::Scalar>,
        options: OverlayOptions<P::Scalar>,
    ) -> Shapes<P> {
        match OutlineSolver::<P, i32>::prepare(self, style) {
            Some(solver) => solver.build(self, options),
            None => vec![],
        }
    }

    fn outline_custom_into(
        &self,
        style: &OutlineStyle<P::Scalar>,
        options: OverlayOptions<P::Scalar>,
        output: &mut FloatFlatContoursBuffer<P>,
    ) {
        match OutlineSolver::<P, i32>::prepare(self, style) {
            Some(solver) => solver.build_into(self, options, output),
            None => output.clear_and_reserve(0, 0),
        }
    }

    fn outline_fixed_scale(
        &self,
        style: &OutlineStyle<P::Scalar>,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError> {
        self.outline_custom_fixed_scale(style, Default::default(), scale)
    }

    fn outline_fixed_scale_into(
        &self,
        style: &OutlineStyle<P::Scalar>,
        scale: P::Scalar,
        output: &mut FloatFlatContoursBuffer<P>,
    ) -> Result<(), FixedScaleOverlayError> {
        self.outline_custom_fixed_scale_into(style, Default::default(), scale, output)
    }

    fn outline_custom_fixed_scale(
        &self,
        style: &OutlineStyle<P::Scalar>,
        options: OverlayOptions<P::Scalar>,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError> {
        let s = FixedScaleOverlayError::validate_scale(scale)?;
        let mut solver = match OutlineSolver::<P, i32>::prepare(self, style) {
            Some(solver) => solver,
            None => return Ok(vec![]),
        };
        solver.apply_scale(s)?;
        Ok(solver.build(self, options))
    }

    fn outline_custom_fixed_scale_into(
        &self,
        style: &OutlineStyle<P::Scalar>,
        options: OverlayOptions<P::Scalar>,
        scale: P::Scalar,
        output: &mut FloatFlatContoursBuffer<P>,
    ) -> Result<(), FixedScaleOverlayError> {
        let s = FixedScaleOverlayError::validate_scale(scale)?;
        let mut solver = match OutlineSolver::<P, i32>::prepare(self, style) {
            Some(solver) => solver,
            None => {
                output.clear_and_reserve(0, 0);
                return Ok(());
            }
        };
        solver.apply_scale(s)?;
        solver.build_into(self, options, output);
        Ok(())
    }

    fn outline_as<I>(&self, style: &OutlineStyle<P::Scalar>) -> Shapes<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static,
    {
        self.outline_custom_as::<I>(style, Default::default())
    }

    fn outline_into_as<I>(&self, style: &OutlineStyle<P::Scalar>, output: &mut FloatFlatContoursBuffer<P>)
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static,
    {
        self.outline_custom_into_as::<I>(style, Default::default(), output)
    }

    fn outline_custom_as<I>(
        &self,
        style: &OutlineStyle<P::Scalar>,
        options: OverlayOptions<P::Scalar, I>,
    ) -> Shapes<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static,
    {
        match OutlineSolver::<P, I>::prepare(self, style) {
            Some(solver) => solver.build(self, options),
            None => vec![],
        }
    }

    fn outline_custom_into_as<I>(
        &self,
        style: &OutlineStyle<P::Scalar>,
        options: OverlayOptions<P::Scalar, I>,
        output: &mut FloatFlatContoursBuffer<P>,
    ) where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static,
    {
        match OutlineSolver::<P, I>::prepare(self, style) {
            Some(solver) => solver.build_into(self, options, output),
            None => output.clear_and_reserve(0, 0),
        }
    }

    fn outline_fixed_scale_as<I>(
        &self,
        style: &OutlineStyle<P::Scalar>,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static,
    {
        self.outline_custom_fixed_scale_as::<I>(style, Default::default(), scale)
    }

    fn outline_fixed_scale_into_as<I>(
        &self,
        style: &OutlineStyle<P::Scalar>,
        scale: P::Scalar,
        output: &mut FloatFlatContoursBuffer<P>,
    ) -> Result<(), FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static,
    {
        self.outline_custom_fixed_scale_into_as::<I>(style, Default::default(), scale, output)
    }

    fn outline_custom_fixed_scale_as<I>(
        &self,
        style: &OutlineStyle<P::Scalar>,
        options: OverlayOptions<P::Scalar, I>,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static,
    {
        let s = FixedScaleOverlayError::validate_scale(scale)?;
        let mut solver = match OutlineSolver::<P, I>::prepare(self, style) {
            Some(solver) => solver,
            None => return Ok(vec![]),
        };
        solver.apply_scale(s)?;
        Ok(solver.build(self, options))
    }

    fn outline_custom_fixed_scale_into_as<I>(
        &self,
        style: &OutlineStyle<P::Scalar>,
        options: OverlayOptions<P::Scalar, I>,
        scale: P::Scalar,
        output: &mut FloatFlatContoursBuffer<P>,
    ) -> Result<(), FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static,
    {
        let s = FixedScaleOverlayError::validate_scale(scale)?;
        let mut solver = match OutlineSolver::<P, I>::prepare(self, style) {
            Some(solver) => solver,
            None => {
                output.clear_and_reserve(0, 0);
                return Ok(());
            }
        };
        solver.apply_scale(s)?;
        solver.build_into(self, options, output);
        Ok(())
    }
}

struct OutlineSolver<P: FloatPointCompatible, I: IntNumber> {
    outer_builder: OutlineBuilder<P, I>,
    inner_builder: OutlineBuilder<P, I>,
    adapter: FloatPointAdapter<P, I>,
    points_count: usize,
}

impl<P, I> OutlineSolver<P, I>
where
    P: FloatPointCompatible + 'static,
    I: IntNumber + Expiration + LayoutNumber + SortKey + 'static,
{
    fn prepare<S: ShapeResource<P>>(source: &S, style: &OutlineStyle<P::Scalar>) -> Option<Self> {
        let (points_count, paths_count) = {
            let mut points_count = 0;
            let mut paths_count = 0;
            for path in source.iter_paths() {
                points_count += path.len();
                paths_count += 1;
            }
            (points_count, paths_count)
        };

        if paths_count == 0 {
            return None;
        }

        let join = style.join.clone().normalize();
        let outer_builder: OutlineBuilder<P, I> = OutlineBuilder::new(-style.outer_offset, &join);
        let inner_builder: OutlineBuilder<P, I> = OutlineBuilder::new(-style.inner_offset, &join);

        let outer_radius = style.outer_offset;
        let inner_radius = style.inner_offset;

        let outer_additional_offset = outer_builder.additional_offset(outer_radius);
        let inner_additional_offset = inner_builder.additional_offset(inner_radius);

        let additional_offset = outer_additional_offset.abs() + inner_additional_offset.abs();

        let mut rect = FloatRect::with_iter(source.iter_paths().flatten()).unwrap_or(FloatRect::zero());
        rect.add_offset(additional_offset);

        let adapter = FloatPointAdapter::<P, I>::new(rect);

        Some(Self {
            outer_builder,
            inner_builder,
            adapter,
            points_count,
        })
    }

    fn apply_scale(&mut self, scale: f64) -> Result<(), FixedScaleOverlayError> {
        let s = P::Scalar::from_float(scale);
        self.adapter = FloatPointAdapter::try_with_scale(*self.adapter.rect(), s)?;
        Ok(())
    }

    fn build_overlay<S: ShapeResource<P>>(
        &self,
        source: &S,
        options: OverlayOptions<P::Scalar, I>,
    ) -> Overlay<I> {
        let total_capacity = self.outer_builder.capacity(self.points_count);
        let mut overlay = Overlay::new_custom(
            total_capacity,
            options.int_with_adapter(&self.adapter),
            Default::default(),
        );

        let mut offset_overlay = Overlay::new(16);
        offset_overlay.options = overlay.options;

        let mut segments = Vec::new();
        let mut bool_buffer = BooleanExtractionBuffer::default();
        let mut flat_buffer = FlatContoursBuffer::<I>::with_capacity(0);

        for path in source.iter_paths() {
            let area = path.unsafe_int_area(&self.adapter);
            if area.unsigned_abs() <= <I::WideUInt as UIntNumber>::from_u64(1) {
                // ignore degenerate paths
                continue;
            }

            offset_overlay.clear();
            segments.clear();

            let contour_fill_rule = if area > I::Wide::ZERO {
                offset_overlay.options.output_direction = ContourDirection::CounterClockwise;
                segments.reserve(self.outer_builder.capacity(path.len()));
                self.outer_builder.build(path, &self.adapter, &mut segments);
                FillRule::Positive
            } else {
                offset_overlay.options.output_direction = ContourDirection::Clockwise;
                segments.reserve(self.inner_builder.capacity(path.len()));
                self.inner_builder.build(path, &self.adapter, &mut segments);

                FillRule::Negative
            };

            offset_overlay.add_segments(&segments);

            if let Some(graph) = offset_overlay.build_graph_view(contour_fill_rule) {
                graph.extract_contours_into(OverlayRule::Subject, &mut bool_buffer, &mut flat_buffer);
            }

            overlay.add_flat_buffer(&flat_buffer, Subject);
        }

        overlay
    }

    fn build<S: ShapeResource<P>>(self, source: &S, options: OverlayOptions<P::Scalar, I>) -> Shapes<P> {
        let preserve_output_collinear = options.preserve_output_collinear;
        let clean_result = options.clean_result;
        let mut overlay = self.build_overlay(source, options);
        let shapes = overlay.overlay(OverlayRule::Subject, FillRule::Positive);

        if clean_result {
            let mut float = shapes.to_float(&self.adapter);
            if preserve_output_collinear {
                float.despike_contour(&self.adapter);
            } else {
                float.simplify_contour(&self.adapter);
            }
            float
        } else {
            shapes.to_float(&self.adapter)
        }
    }

    fn build_into<S: ShapeResource<P>>(
        self,
        source: &S,
        options: OverlayOptions<P::Scalar, I>,
        output: &mut FloatFlatContoursBuffer<P>,
    ) {
        let preserve_output_collinear = options.preserve_output_collinear;
        let clean_result = options.clean_result;
        let mut overlay = self.build_overlay(source, options);

        let mut int_output = FlatContoursBuffer::<I>::with_capacity(0);
        overlay.overlay_into(OverlayRule::Subject, FillRule::Positive, &mut int_output);
        let iter = int_output.points.iter().map(|p| self.adapter.int_to_float(p));
        output.set_with_iter(iter, &int_output.ranges);

        if clean_result {
            if preserve_output_collinear {
                output.despike_contour(&self.adapter);
            } else {
                output.simplify_contour(&self.adapter);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::core::fill_rule::FillRule;
    use crate::float::simplify::SimplifyShape;
    use crate::mesh::outline::offset::OutlineOffset;
    use crate::mesh::style::{LineJoin, OutlineStyle};
    use alloc::vec;
    use alloc::vec::Vec;
    use core::f32::consts::PI;
    use i_shape::base::data::{Path, Shape};
    use i_shape::flat::float::FloatFlatContoursBuffer;
    use i_shape::float::area::Area;
    use rand::RngExt;

    #[test]
    fn test_doc() {
        let shape = vec![
            vec![
                [2.0, 1.0],
                [4.0, 1.0],
                [5.0, 2.0],
                [13.0, 2.0],
                [13.0, 3.0],
                [12.0, 3.0],
                [12.0, 4.0],
                [11.0, 4.0],
                [11.0, 3.0],
                [10.0, 3.0],
                [9.0, 4.0],
                [8.0, 4.0],
                [8.0, 3.0],
                [5.0, 3.0],
                [5.0, 4.0],
                [4.0, 5.0],
                [2.0, 5.0],
                [1.0, 4.0],
                [1.0, 2.0],
            ],
            vec![[2.0, 4.0], [4.0, 4.0], [4.0, 2.0], [2.0, 2.0]],
        ];

        let style = OutlineStyle::new(0.2).line_join(LineJoin::Round(0.1));
        let shapes = shape.outline(&style);

        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 2);
    }

    #[test]
    fn test_triangle_round_corner() {
        let path = [[0.0, 0.0f32], [10.0, 0.0f32], [0.0, 10.0f32]];

        let style = OutlineStyle::new(5.0).line_join(LineJoin::Round(0.25 * PI));
        let shapes = path.outline(&style);

        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 1);
    }

    #[test]
    fn test_triangle_round_corner_as() {
        let path = [[0.0, 0.0f32], [10.0, 0.0f32], [0.0, 10.0f32]];

        let style = OutlineStyle::new(5.0).line_join(LineJoin::Round(0.25 * PI));
        let shapes = path.outline_as::<i64>(&style);

        assert_eq!(shapes.len(), 1);
    }

    #[test]
    fn test_reversed_triangle_round_corner() {
        let path = [[0.0, 0.0f32], [0.0, 10.0f32], [10.0, 0.0f32]];

        let style = OutlineStyle::new(5.0).line_join(LineJoin::Round(0.25 * PI));
        let shapes = path.outline(&style);

        assert_eq!(shapes.len(), 0);
    }

    #[test]
    fn test_square_zero_offset() {
        let path = [[-5.0, -5.0f32], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]];

        let style = OutlineStyle::new(0.0);
        let shapes = path.outline_fixed_scale(&style, 10.0).unwrap();

        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 1);

        let path = shape.first().unwrap();
        assert_eq!(path.len(), 4);
    }

    #[test]
    fn test_outline_into_ok() {
        let path = [[-5.0, -5.0f32], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]];
        let style = OutlineStyle::new(1.0);
        let mut output = FloatFlatContoursBuffer::default();

        path.outline_into(&style, &mut output);

        assert!(!output.is_empty());
        assert!(!output.ranges.is_empty());
    }

    #[test]
    fn test_outline_fixed_scale_into_ok() {
        let path = [[-5.0, -5.0f32], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]];
        let style = OutlineStyle::new(1.0);
        let mut output = FloatFlatContoursBuffer::default();

        path.outline_fixed_scale_into(&style, 10.0, &mut output).unwrap();

        assert!(!output.is_empty());
        assert!(!output.ranges.is_empty());
    }

    #[test]
    fn test_square_positive_offset_0() {
        let path = [[-5.0, -5.0f32], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]];
        let original_sign = path.area().signum();

        let style = OutlineStyle::new(1.0);
        let shapes = path.outline_fixed_scale(&style, 10.0).unwrap();

        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 1);

        let path = shape.first().unwrap();
        assert_eq!(path.len(), 8);

        let result_sign = path.area().signum();
        assert_eq!(original_sign, result_sign);
    }

    #[test]
    fn test_square_positive_offset_1() {
        let path = [[-5.0, -5.0f32], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]];
        let original_sign = path.area().signum();

        let style = OutlineStyle::new(10.0);
        let shapes = path.outline_fixed_scale(&style, 10.0).unwrap();

        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 1);

        let path = shape.first().unwrap();
        assert_eq!(path.len(), 8);

        let result_sign = path.area().signum();
        assert_eq!(original_sign, result_sign);
    }

    #[test]
    fn test_square_round_offset() {
        let path = [[-5.0, -5.0f32], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]];
        let original_sign = path.area().signum();

        let angle = PI / 3.0f32;
        let style = OutlineStyle::new(10.0).line_join(LineJoin::Round(angle));

        let shapes = path.outline_fixed_scale(&style, 10.0).unwrap();

        assert_eq!(shapes.len(), 1);

        let result_sign = path.area().signum();
        assert_eq!(original_sign, result_sign);
    }

    #[test]
    fn test_square_negative_offset_0() {
        let path = [[-5.0, -5.0f32], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]];
        let original_sign = path.area().signum();

        let style = OutlineStyle::new(-1.0);
        let shapes = path.outline_fixed_scale(&style, 10.0).unwrap();

        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 1);

        let path = shape.first().unwrap();
        assert_eq!(path.len(), 4);

        let result_sign = path.area().signum();
        assert_eq!(original_sign, result_sign);
    }

    #[test]
    fn test_square_negative_offset_1() {
        let path = [[-5.0, -5.0f32], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]];

        let style = OutlineStyle::new(-6.0);
        let shapes = path.outline_fixed_scale(&style, 10.0).unwrap();

        assert_eq!(shapes.len(), 0);
    }

    #[test]
    fn test_square_negative_offset_2() {
        let path = [[-5.0, -5.0f32], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]];

        let style = OutlineStyle::new(-10.0);
        let shapes = path.outline_fixed_scale(&style, 10.0).unwrap();

        assert_eq!(shapes.len(), 0);
    }

    #[test]
    fn test_square_negative_offset_3() {
        let path = [[-5.0, -5.0f32], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]];

        let style = OutlineStyle::new(-11.0);
        let shapes = path.outline_fixed_scale(&style, 10.0).unwrap();

        assert_eq!(shapes.len(), 0);
    }

    #[test]
    fn test_square_negative_round_offset() {
        let path = [[-5.0, -5.0f32], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]];

        let angle = PI / 3.0f32;
        let style = OutlineStyle::new(-20.0).line_join(LineJoin::Round(angle));

        let shapes = path.outline_fixed_scale(&style, 10.0).unwrap();

        assert_eq!(shapes.len(), 0);
    }

    #[test]
    fn test_square_positive_miter_offset() {
        let path = [[-5.0, -5.0f32], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]];
        let original_sign = path.area().signum();

        let style = OutlineStyle::new(1.0).line_join(LineJoin::Miter(0.01));

        let shapes = path.outline_fixed_scale(&style, 10.0).unwrap();

        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 1);

        let path = shape.first().unwrap();
        assert_eq!(path.len(), 4);

        let result_sign = path.area().signum();
        assert_eq!(original_sign, result_sign);
    }

    #[test]
    fn test_inner_corner_positive_offset_0() {
        let path = [
            [-5.0, 5.0],
            [-5.0, -5.0],
            [5.0, -5.0],
            [5.0, 0.0],
            [0.0, 0.0],
            [0.0, 5.0f32],
        ];
        let original_sign = path.area().signum();

        let style = OutlineStyle::new(1.0);

        let shapes = path.outline_fixed_scale(&style, 10.0).unwrap();

        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 1);

        let path = shape.first().unwrap();
        assert_eq!(path.len(), 11);

        let result_sign = path.area().signum();
        assert_eq!(original_sign, result_sign);
    }

    #[test]
    fn test_inner_corner_positive_offset_1() {
        let path = [
            [-5.0, 5.0],
            [-5.0, -5.0],
            [5.0, -5.0],
            [5.0, 0.0],
            [0.0, 0.0],
            [0.0, 5.0f32],
        ];
        let original_sign = path.area().signum();

        let style = OutlineStyle::new(5.0);

        let shapes = path.outline_fixed_scale(&style, 10.0).unwrap();

        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 1);

        let path = shape.first().unwrap();
        assert_eq!(path.len(), 8);

        let result_sign = path.area().signum();
        assert_eq!(original_sign, result_sign);
    }

    #[test]
    fn test_inner_corner_positive_offset_2() {
        let path = [
            [-5.0, 5.0],
            [-5.0, -5.0],
            [5.0, -5.0],
            [5.0, 0.0],
            [0.0, 0.0],
            [0.0, 5.0f32],
        ];
        let original_sign = path.area().signum();

        let style = OutlineStyle::new(20.0);

        let shapes = path.outline_fixed_scale(&style, 10.0).unwrap();

        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 1);

        let path = shape.first().unwrap();
        assert_eq!(path.len(), 8);

        let result_sign = path.area().signum();
        assert_eq!(original_sign, result_sign);
    }

    #[test]
    fn test_inner_corner_negative_offset_0() {
        let path = [
            [-5.0, 5.0],
            [-5.0, -5.0],
            [5.0, -5.0],
            [5.0, 0.0],
            [0.0, 0.0],
            [0.0, 5.0f32],
        ];
        let original_sign = path.area().signum();

        let style = OutlineStyle::new(-1.0);

        let shapes = path.outline_fixed_scale(&style, 10.0).unwrap();

        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 1);

        let path = shape.first().unwrap();
        assert_eq!(path.len(), 7);

        let result_sign = path.area().signum();
        assert_eq!(original_sign, result_sign);
    }

    #[test]
    fn test_inner_corner_negative_offset_1() {
        let path = [
            [-5.0, 5.0],
            [-5.0, -5.0],
            [5.0, -5.0],
            [5.0, 0.0],
            [0.0, 0.0],
            [0.0, 5.0f32],
        ];

        let style = OutlineStyle::new(-5.0);

        let shapes = path.outline_fixed_scale(&style, 10.0).unwrap();

        assert_eq!(shapes.len(), 0);
    }

    #[test]
    fn test_rhombus_miter() {
        let path = [[-10.0, 0.0], [0.0, -10.0], [10.0, 0.0], [0.0, 10.0]];

        let style = OutlineStyle::new(5.0).line_join(LineJoin::Miter(0.01));
        let shapes = path.outline_fixed_scale(&style, 10.0).unwrap();

        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes.first().unwrap().len(), 1);
    }

    #[test]
    fn test_window() {
        let window = vec![
            vec![[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [-10.0, 10.0]],
            vec![[-5.0, -5.0], [-5.0, 5.0], [5.0, 5.0], [5.0, -5.0]],
        ];

        let style = OutlineStyle::new(1.0).line_join(LineJoin::Bevel);
        let shapes = window.outline_fixed_scale(&style, 10.0).unwrap();

        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].len(), 2);
        assert_eq!(shapes[0][0].len(), 8);
        assert_eq!(shapes[0][1].len(), 4);
    }

    #[test]
    fn test_float_square_0() {
        let shape = vec![vec![
            [300.0, 300.0],
            [500.0, 300.0],
            [500.0, 500.0],
            [300.0, 500.0],
        ]];

        let style = OutlineStyle::default().outer_offset(50.0).inner_offset(50.0);

        let shapes = shape.outline_fixed_scale(&style, 10.0).unwrap();

        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 1);

        let path = shape.first().unwrap();
        assert_eq!(path.len(), 8);
    }

    #[test]
    fn test_outline_fixed_scale_ok() {
        let path = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let style = OutlineStyle::new(1.0);

        let shapes = path.outline_fixed_scale(&style, 10.0).unwrap();

        assert_eq!(shapes.len(), 1);
    }

    #[test]
    fn test_outline_fixed_scale_invalid() {
        let path = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let style = OutlineStyle::new(1.0);

        assert!(path.outline_fixed_scale(&style, 0.0).is_err());
        assert!(path.outline_fixed_scale(&style, -1.0).is_err());
        assert!(path.outline_fixed_scale(&style, f64::NAN).is_err());
        assert!(path.outline_fixed_scale(&style, f64::INFINITY).is_err());
    }

    #[test]
    fn test_degenerate_0() {
        let path = [[-10.0, 10.0], [-10.0, -10.0], [10.0, -10.0], [10.0, 10.0f32]];
        let original_sign = path.area().signum();

        let style = OutlineStyle::new(0.1);
        let shapes = path.outline_fixed_scale(&style, 1.0).unwrap();

        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 1);

        let path = shape.first().unwrap();
        assert_eq!(path.len(), 4);

        let result_sign = path.area().signum();
        assert_eq!(original_sign, result_sign);
    }

    #[test]
    fn test_degenerate_1() {
        let path = [[-10.0, 10.0], [-10.0, -10.0], [10.0, -10.0], [10.0, 10.0f32]];
        let original_sign = path.area().signum();

        let style = OutlineStyle::new(1.0).line_join(LineJoin::Miter(0.01));
        let shapes = path.outline_fixed_scale(&style, 1.0).unwrap();

        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 1);

        let path = shape.first().unwrap();
        assert_eq!(path.len(), 8);

        let result_sign = path.area().signum();
        assert_eq!(original_sign, result_sign);
    }

    #[test]
    fn test_zero_length_segment_0() {
        let path = [
            [2681.39599938213, 5892784.488998892],
            [5419.06964821636, 5891947.742386343],
            [5419.1446127397, 5891949.316633703],
            [5422.8669123155, 5892027.484991552],
            [5034.8682417375, 5892817.151239874],
            [4804.8188261491, 5892876.799252035],
            [4804.81882805645, 5892876.799253942],
            [4551.3436274034, 5892942.5211854],
            [2681.39599938213, 5892784.488998892],
        ];

        let angle = 10.0f64 / (core::f64::consts::PI / 2.0f64);
        let style = OutlineStyle::new(150.0).line_join(LineJoin::Round(angle));

        if let Some(shape) = path.outline(&style).first() {
            assert!(shape[0].len() < 1_000);
        };
    }

    #[test]
    fn test_zero_length_segment_1() {
        let path = [
            [2681.39599938213, 5892876.0],
            [5400.0, 5891947.742386343],
            [5400.0, 5892817.151239874],
            [4804.8188261491, 5892876.799252035],
            [4804.81882805645, 5892876.799253942],
        ];

        let angle = 10.0f64 / (core::f64::consts::PI / 2.0f64);
        let style = OutlineStyle::new(150.0).line_join(LineJoin::Round(angle));

        if let Some(shape) = path.outline(&style).first() {
            assert!(shape[0].len() < 1_000);
        };
    }

    #[test]
    fn test_star_bevel_0() {
        let r0 = 5.0;
        let r1 = 20.0;
        let pi = core::f64::consts::PI;
        for count in 8..24 {
            let mut angle = 0.0;
            while angle < pi {
                let shape = create_star(r0, r1, count, angle);
                let mut offset = 0.0;
                let mut prev_area = 0.0;
                while offset < 10.0 {
                    let min_area = pi * (r0 + offset).powi(2);
                    let max_area = pi * (r1 + offset).powi(2);

                    let style = OutlineStyle::new(offset);
                    let outline_shapes = shape.outline(&style);
                    let area = outline_shapes.area();

                    assert!(area >= min_area);
                    assert!(area <= max_area);
                    assert!(prev_area < area);

                    offset += 0.5;
                    prev_area = area;
                }
                angle += 1.0;
            }
        }
    }

    #[test]
    fn test_star_round_0() {
        let r0 = 5.0;
        let r1 = 20.0;
        let pi = core::f64::consts::PI;
        let join_angle = pi / 3.0;
        for count in 8..24 {
            let mut angle = 0.0;
            while angle < pi {
                let shape = create_star(r0, r1, count, angle);
                let mut offset = 0.0;
                let mut prev_area = 0.0;
                while offset < 10.0 {
                    let min_area = pi * (r0 + offset).powi(2);
                    let max_area = pi * (r1 + offset).powi(2);
                    let style = OutlineStyle::new(offset).line_join(LineJoin::Round(join_angle));

                    let outline_shapes = shape.outline(&style);
                    let area = outline_shapes.area();

                    assert!(area >= min_area);
                    assert!(area <= max_area);
                    assert!(prev_area < area);

                    offset += 0.5;
                    prev_area = area;
                }
                angle += 1.0;
            }
        }
    }

    #[test]
    fn test_random_0() {
        let style = OutlineStyle::new(10.0);
        for _ in 0..100 {
            let shapes = random_float(100.0, 100).simplify_shape(FillRule::NonZero);
            let base_area = shapes.area();
            let outline_shapes = shapes.outline(&style);
            let area = outline_shapes.area();
            assert!(base_area < area);
        }
    }

    #[test]
    fn test_random_1() {
        let join_angle = core::f64::consts::PI / 3.0;
        let style = OutlineStyle::new(10.0).line_join(LineJoin::Round(join_angle));
        for _ in 0..100 {
            let shapes = random_float(100.0, 100).simplify_shape(FillRule::NonZero);
            let base_area = shapes.area();
            let outline_shapes = shapes.outline(&style);
            let area = outline_shapes.area();
            assert!(base_area < area);
        }
    }

    #[test]
    fn test_random_2() {
        let style = OutlineStyle::new(-10.0);
        for _ in 0..100 {
            let shapes = random_float(100.0, 100).simplify_shape(FillRule::NonZero);
            let base_area = shapes.area();
            let outline_shapes = shapes.outline(&style);
            let area = outline_shapes.area();
            assert!(base_area >= area);
        }
    }

    #[test]
    fn test_random_3() {
        let join_angle = core::f64::consts::PI / 3.0;
        let style = OutlineStyle::new(-10.0).line_join(LineJoin::Round(join_angle));
        for _ in 0..100 {
            let shapes = random_float(100.0, 100).simplify_shape(FillRule::NonZero);
            let base_area = shapes.area();
            let outline_shapes = shapes.outline(&style);
            let area = outline_shapes.area();
            assert!(base_area >= area);
        }
    }

    #[test]
    fn test_real_case_0() {
        let main = vec![
            [411162.0470393328, 5848155.806033095],
            [411162.3299983172, 5848152.285037002],
            [411162.44901687186, 5848149.446047744],
            [411167.5609553484, 5848148.9709500875],
            [411175.2629817156, 5848147.891970595],
            [411186.7560237078, 5848146.501955947],
            [411203.86503249686, 5848144.432009658],
            [411214.44804030936, 5848143.314944228],
            [411221.0470393328, 5848142.421999892],
            [411227.85697585624, 5848141.499026259],
            [411233.74100905936, 5848140.505007705],
            [411238.4249690203, 5848139.349978408],
            [411242.85697585624, 5848138.25305458],
            [411249.1400569109, 5848136.395022353],
            [411256.6129573015, 5848134.406008681],
            [411262.81803542655, 5848132.916018447],
            [411275.2460139422, 5848129.93298622],
            [411284.6999934344, 5848127.662966689],
            [411292.3739436297, 5848125.3869657125],
            [411295.41703445, 5848123.3430204],
            [411297.0079768328, 5848121.340945205],
            [411297.43900710624, 5848119.1510037985],
            [411293.11698562186, 5848105.54602333],
            [411287.24100905936, 5848076.412966689],
            [411286.6709407, 5848062.798953017],
            [411286.98099929374, 5848053.410037002],
            [411288.3879817156, 5848038.451052627],
            [411294.0620539813, 5848006.396975478],
            [411294.9409602312, 5847995.477053603],
            [411295.2140315203, 5847988.534060439],
            [411296.3359797625, 5847983.056033095],
            [411297.8600276141, 5847976.624026259],
            [411297.86698562186, 5847976.590945205],
            [411298.2679865984, 5847974.952029189],
            [411301.5709651141, 5847965.958010634],
            [411303.7980158953, 5847955.29602333],
            [411305.12797195, 5847948.927004775],
            [411307.15897780936, 5847937.4279813375],
            [411307.711956325, 5847934.313967666],
            [411310.8889582781, 5847916.500979384],
            [411311.8309748797, 5847911.5959500875],
            [411312.3839533953, 5847898.51098915],
            [411311.64603835624, 5847891.3459500875],
            [411308.97904616874, 5847878.494997939],
            [411303.793987575, 5847862.650027236],
            [411301.5509455828, 5847857.5080594625],
            [411297.4499934344, 5847849.958010634],
            [411294.81303054374, 5847846.331057509],
            [411281.3550227312, 5847828.88598915],
            [411261.0709651141, 5847805.2369412985],
            [411259.2100032, 5847804.3619412985],
            [411258.0150569109, 5847803.80005165],
            [411254.8910334734, 5847803.62805458],
            [411251.86503249686, 5847805.3430204],
            [411249.4499934344, 5847802.375002822],
            [411248.2519953875, 5847800.202029189],
            [411248.1909602312, 5847794.432009658],
            [411253.23197585624, 5847785.8170194235],
            [411255.7970393328, 5847788.224978408],
            [411257.6870539813, 5847789.534060439],
            [411259.8690608172, 5847790.333010634],
            [411262.0520442156, 5847790.332034072],
            [411268.8910334734, 5847788.8420438375],
            [411269.4320490984, 5847790.333010634],
            [411270.6329768328, 5847793.6369657125],
            [411269.1820490984, 5847794.332034072],
            [411267.65397292655, 5847796.224001845],
            [411266.9259455828, 5847798.990969619],
            [411267.6709407, 5847800.479006728],
            [411268.7440608172, 5847802.625979384],
            [411281.10795241874, 5847816.8850125875],
            [411283.4420588641, 5847819.822024306],
            [411294.9740412859, 5847834.3430204],
            [411304.0699885515, 5847846.6369657125],
            [411307.27103835624, 5847852.748049697],
            [411309.8900569109, 5847857.912966689],
            [411311.78300124686, 5847864.460940322],
            [411313.6019709734, 5847869.698977431],
            [411314.9850276141, 5847872.171999892],
            [411317.53104812186, 5847875.446047744],
            [411320.7970393328, 5847877.480959853],
            [411325.86600905936, 5847879.2180204],
            [411335.5499690203, 5847882.012942275],
            [411368.4549983172, 5847890.26098915],
            [411387.668987575, 5847895.4010037985],
            [411397.7240412859, 5847898.576052627],
            [411405.50297195, 5847902.333010634],
            [411411.0599787859, 5847905.931033095],
            [411418.5199397234, 5847911.750979384],
            [411434.2660334734, 5847926.797976455],
            [411436.82304030936, 5847934.838015517],
            [411437.5780451922, 5847936.113039931],
            [411434.0089533953, 5847946.812991103],
            [411431.19804030936, 5847949.901980361],
            [411411.5659602312, 5847985.880984267],
            [411407.6529963641, 5847993.0510282125],
            [411404.77994948905, 5848000.453982314],
            [411402.93803054374, 5848007.354983291],
            [411399.39701491874, 5848029.913943252],
            [411392.9289973406, 5848080.029055556],
            [411390.43998366874, 5848099.298953017],
            [411388.8789485125, 5848106.744021377],
            [411386.1429865984, 5848113.750979384],
            [411383.2870295672, 5848120.22595497],
            [411379.1269953875, 5848126.2580594625],
            [411373.0499690203, 5848132.165041884],
            [411368.44901687186, 5848135.741946181],
            [411362.3199885515, 5848138.749026259],
            [411354.7980158953, 5848141.105959853],
            [411345.8729670672, 5848143.8990506735],
            [411334.5969660906, 5848146.394045791],
            [411322.7279475359, 5848149.957034072],
            [411321.0050471453, 5848151.457034072],
            [411319.7229426531, 5848152.791995009],
            [411319.23698073905, 5848154.2740506735],
            [411319.336956325, 5848156.656008681],
            [411339.95194655936, 5848207.255007705],
            [411351.7620051531, 5848236.444949111],
            [411364.9020198015, 5848268.477053603],
            [411376.5170100359, 5848297.156008681],
            [411377.7340510515, 5848300.22595497],
            [411395.8690608172, 5848345.97595497],
            [411411.2689631609, 5848381.8459500875],
            [411413.1310237078, 5848382.543948134],
            [411405.27994948905, 5848384.8010282125],
            [411332.1410334734, 5848206.078005752],
            [411309.5890315203, 5848150.895998916],
            [411307.8690608172, 5848147.239993056],
            [411305.3419612078, 5848144.284060439],
            [411301.6329768328, 5848141.729983291],
            [411296.9740412859, 5848139.974978408],
            [411293.77494460624, 5848139.145022353],
            [411290.1350520281, 5848139.240969619],
            [411276.68998366874, 5848140.473025283],
            [411274.44901687186, 5848140.531008681],
            [411266.961956325, 5848144.207034072],
            [411247.6239436297, 5848146.937014541],
            [411246.2670100359, 5848147.2369412985],
            [411240.0699885515, 5848148.041018447],
            [411234.7219660906, 5848150.973025283],
            [411224.2479670672, 5848149.892947158],
            [411223.6759455828, 5848148.834963759],
            [411222.2269709734, 5848148.297976455],
            [411213.2560237078, 5848149.380984267],
            [411189.6649592547, 5848152.199953994],
            [411162.0470393328, 5848155.806033095],
        ];

        let hole = vec![
            [411294.2500422625, 5848072.3189725485],
            [411373.9180110125, 5848124.016970595],
            [411377.22904616874, 5848118.990969619],
            [411393.0859797625, 5848020.979983291],
            [411394.7030451922, 5848005.639040908],
            [411397.4359553484, 5848003.376955947],
            [411431.1029475359, 5847937.537966689],
            [411431.2639582781, 5847933.187991103],
            [411314.8390315203, 5848005.308962783],
            [411314.5590022234, 5848009.708010634],
            [411309.3459895281, 5848009.447024306],
            [411305.3719905047, 5848010.244997939],
            [411294.2500422625, 5848072.3189725485],
        ];

        let shape = vec![main, hole];

        let angle = 10.0f64 / (core::f64::consts::PI / 2.0f64);
        let style = OutlineStyle::new(600.0).line_join(LineJoin::Round(angle));

        if let Some(shape) = shape.outline(&style).first() {
            assert!(shape[0].len() < 1_000);
        };
    }

    #[test]
    fn test_real_case_0_simplified() {
        let main = vec![
            [410_000.0, 5_847_000.0],
            [413_000.0, 5_847_000.0],
            [413_000.0, 5_850_000.0],
            [410_000.0, 5_850_000.0],
        ];

        let hole = vec![
            [411_294.2500422625, 5_848_072.318_972_548_5],
            [411_373.9180110125, 5_848_124.016_970_595],
            [411_377.22904616874, 5_848_118.990_969_619],
            [411_393.0859797625, 5_848_020.979_983_291],
            [411_394.7030451922, 5_848_005.639_040_908],
            [411_397.4359553484, 5_848_003.376_955_947],
            [411_431.1029475359, 5_847_937.537_966_689],
            [411_431.2639582781, 5_847_933.187_991_103],
            [411_314.8390315203, 5_848_005.308_962_783],
            [411_314.5590022234, 5_848_009.708_010_634],
            [411_309.3459895281, 5_848_009.447_024_306],
            [411_305.3719905047, 5_848_010.244_997_939],
        ];

        let shape = vec![main, hole];

        let angle = 10.0f64 / (core::f64::consts::PI / 2.0f64);
        let style = OutlineStyle::new(600.0).line_join(LineJoin::Round(angle));

        if let Some(shape) = shape.outline(&style).first() {
            assert!(shape[0].len() < 1_000);
        };
    }

    #[test]
    fn test_real_case_1_offset() {
        let contour = vec![
            [53.0, 42.0],
            [35.0, 66.0],
            [26.0, 75.0],
            [27.0, 74.0],
            [53.0f64, 42.0],
        ];

        let style = OutlineStyle::new(1.0f64).line_join(LineJoin::Round(1.0f64));
        contour.outline(&style);

        if let Some(shape) = contour.outline(&style).first() {
            assert!(shape[0].len() < 1_000);
        };
    }

    fn create_star(r0: f64, r1: f64, count: usize, angle: f64) -> Shape<[f64; 2]> {
        let da = core::f64::consts::PI / count as f64;
        let mut a = angle;

        let mut points = Vec::new();

        for _ in 0..count {
            let (sn, cs) = a.sin_cos();

            let xr0 = r0 * cs;
            let yr0 = r0 * sn;

            a += da;

            let (sn, cs) = a.sin_cos();
            let xr1 = r1 * cs;
            let yr1 = r1 * sn;

            a += da;

            points.push([xr0, yr0]);
            points.push([xr1, yr1]);
        }

        [points].to_vec()
    }

    fn random_float(radius: f64, n: usize) -> Path<[f64; 2]> {
        let a = 0.5 * radius;
        let range = -a..=a;
        let mut points = Vec::with_capacity(n);
        let mut rng = rand::rng();
        for _ in 0..n {
            let x = rng.random_range(range.clone());
            let y = rng.random_range(range.clone());
            points.push([x, y])
        }

        points
    }
}
