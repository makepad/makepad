use crate::core::fill_rule::FillRule;
use crate::core::overlay::Overlay;
use crate::core::overlay_rule::OverlayRule;
use crate::float::overlay::OverlayOptions;
use crate::float::scale::FixedScaleOverlayError;
use crate::i_shape::source::resource::ShapeResource;
use crate::mesh::stroke::builder::StrokeBuilder;
use crate::mesh::stroke::offset::vec::Vec;
use crate::mesh::style::StrokeStyle;
use alloc::vec;
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
use i_shape::float::simple::SimplifyContour;
use i_tree::{Expiration, LayoutNumber};

/// Trait for generating stroke outlines from float paths.
///
/// Default methods use the `i32` integer engine. Use the `*_as::<I>` methods when you need to
/// select `i16`, `i32`, or `i64` explicitly.
///
/// # Example
///
/// ```
/// use i_overlay::mesh::stroke::offset::StrokeOffset;
/// use i_overlay::mesh::style::StrokeStyle;
///
/// let path = [[0.0, 0.0], [10.0, 0.0]];
/// let style = StrokeStyle::new(2.0);
///
/// let result = path.stroke_as::<i64>(style, false);
///
/// assert_eq!(result.len(), 1);
/// ```
pub trait StrokeOffset<P: FloatPointCompatible> {
    /// Generates a stroke shapes for paths, contours, or shapes.
    ///
    /// - `style`: Defines the stroke properties, including width, line caps, and joins.
    /// - `is_closed_path`: Specifies whether the path is closed (true) or open (false).
    ///
    /// # Returns
    /// A collection of `Shapes<P>` representing the stroke geometry.
    ///
    /// Note: Outer boundary paths have a counterclockwise order, and holes have a clockwise order.
    fn stroke(&self, style: StrokeStyle<P>, is_closed_path: bool) -> Shapes<P>;

    /// Generates stroke contours directly into a flat buffer.
    ///
    /// - `style`: Defines the stroke properties, including width, line caps, and joins.
    /// - `is_closed_path`: Specifies whether the path is closed (true) or open (false).
    /// - `output`: Destination buffer that receives resulting contours. Existing contents are replaced.
    fn stroke_into(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        output: &mut FloatFlatContoursBuffer<P>,
    );

    /// Generates a stroke mesh for paths, contours, or shapes with optional filtering and scaling.
    ///
    /// - `style`: Defines the stroke properties, including width, line caps, and joins.
    /// - `is_closed_path`: Specifies whether the path is closed (true) or open (false).
    /// - `options`: Adjust custom behavior.
    ///
    /// # Returns
    /// A collection of `Shapes<P>` representing the stroke geometry.
    ///
    /// Note: Outer boundary paths have a **main_direction** order, and holes have an opposite to **main_direction** order.
    fn stroke_custom(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        options: OverlayOptions<P::Scalar>,
    ) -> Shapes<P>;

    /// Generates stroke contours directly into a flat buffer with custom overlay options.
    ///
    /// - `style`: Defines the stroke properties, including width, line caps, and joins.
    /// - `is_closed_path`: Specifies whether the path is closed (true) or open (false).
    /// - `options`: Adjust custom behavior.
    /// - `output`: Destination buffer that receives resulting contours. Existing contents are replaced.
    fn stroke_custom_into(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        options: OverlayOptions<P::Scalar>,
        output: &mut FloatFlatContoursBuffer<P>,
    );

    /// Generates a stroke shapes for paths, contours, or shapes with a fixed float-to-integer scale.
    ///
    /// - `style`: Defines the stroke properties, including width, line caps, and joins.
    /// - `is_closed_path`: Specifies whether the path is closed (true) or open (false).
    /// - `scale`: Fixed float-to-integer scale. Use `scale = 1.0 / grid_size` if you prefer grid size semantics.
    ///
    /// # Returns
    /// A collection of `Shapes<P>` representing the stroke geometry.
    ///
    /// Note: Outer boundary paths have a counterclockwise order, and holes have a clockwise order.
    fn stroke_fixed_scale(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError>;

    /// Generates stroke contours directly into a flat buffer with a fixed float-to-integer scale.
    ///
    /// - `style`: Defines the stroke properties, including width, line caps, and joins.
    /// - `is_closed_path`: Specifies whether the path is closed (true) or open (false).
    /// - `scale`: Fixed float-to-integer scale. Use `scale = 1.0 / grid_size` if you prefer grid size semantics.
    /// - `output`: Destination buffer that receives resulting contours. Existing contents are replaced on success.
    fn stroke_fixed_scale_into(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        scale: P::Scalar,
        output: &mut FloatFlatContoursBuffer<P>,
    ) -> Result<(), FixedScaleOverlayError>;

    /// Generates a stroke mesh for paths, contours, or shapes with optional filtering and fixed scaling.
    ///
    /// - `style`: Defines the stroke properties, including width, line caps, and joins.
    /// - `is_closed_path`: Specifies whether the path is closed (true) or open (false).
    /// - `options`: Adjust custom behavior.
    /// - `scale`: Fixed float-to-integer scale. Use `scale = 1.0 / grid_size` if you prefer grid size semantics.
    ///
    /// # Returns
    /// A collection of `Shapes<P>` representing the stroke geometry.
    ///
    /// Note: Outer boundary paths have a **main_direction** order, and holes have an opposite to **main_direction** order.
    fn stroke_custom_fixed_scale(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        options: OverlayOptions<P::Scalar>,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError>;

    /// Generates stroke contours directly into a flat buffer with custom options and fixed scaling.
    ///
    /// - `style`: Defines the stroke properties, including width, line caps, and joins.
    /// - `is_closed_path`: Specifies whether the path is closed (true) or open (false).
    /// - `options`: Adjust custom behavior.
    /// - `scale`: Fixed float-to-integer scale. Use `scale = 1.0 / grid_size` if you prefer grid size semantics.
    /// - `output`: Destination buffer that receives resulting contours. Existing contents are replaced on success.
    fn stroke_custom_fixed_scale_into(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        options: OverlayOptions<P::Scalar>,
        scale: P::Scalar,
        output: &mut FloatFlatContoursBuffer<P>,
    ) -> Result<(), FixedScaleOverlayError>;

    /// Same as [`Self::stroke`], but with an explicit integer engine.
    fn stroke_as<I>(&self, style: StrokeStyle<P>, is_closed_path: bool) -> Shapes<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static;

    /// Same as [`Self::stroke_into`], but with an explicit integer engine.
    fn stroke_into_as<I>(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        output: &mut FloatFlatContoursBuffer<P>,
    ) where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static;

    /// Same as [`Self::stroke_custom`], but with an explicit integer engine.
    fn stroke_custom_as<I>(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        options: OverlayOptions<P::Scalar, I>,
    ) -> Shapes<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static;

    /// Same as [`Self::stroke_custom_into`], but with an explicit integer engine.
    fn stroke_custom_into_as<I>(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        options: OverlayOptions<P::Scalar, I>,
        output: &mut FloatFlatContoursBuffer<P>,
    ) where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static;

    /// Same as [`Self::stroke_fixed_scale`], but with an explicit integer engine.
    fn stroke_fixed_scale_as<I>(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static;

    /// Same as [`Self::stroke_fixed_scale_into`], but with an explicit integer engine.
    fn stroke_fixed_scale_into_as<I>(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        scale: P::Scalar,
        output: &mut FloatFlatContoursBuffer<P>,
    ) -> Result<(), FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static;

    /// Same as [`Self::stroke_custom_fixed_scale`], but with an explicit integer engine.
    fn stroke_custom_fixed_scale_as<I>(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        options: OverlayOptions<P::Scalar, I>,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static;

    /// Same as [`Self::stroke_custom_fixed_scale_into`], but with an explicit integer engine.
    fn stroke_custom_fixed_scale_into_as<I>(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        options: OverlayOptions<P::Scalar, I>,
        scale: P::Scalar,
        output: &mut FloatFlatContoursBuffer<P>,
    ) -> Result<(), FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static;
}

impl<S, P> StrokeOffset<P> for S
where
    S: ShapeResource<P>,
    P: FloatPointCompatible + 'static,
{
    fn stroke(&self, style: StrokeStyle<P>, is_closed_path: bool) -> Shapes<P> {
        self.stroke_custom(style, is_closed_path, Default::default())
    }

    fn stroke_into(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        output: &mut FloatFlatContoursBuffer<P>,
    ) {
        self.stroke_custom_into(style, is_closed_path, Default::default(), output)
    }

    fn stroke_fixed_scale(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError> {
        self.stroke_custom_fixed_scale(style, is_closed_path, Default::default(), scale)
    }

    fn stroke_fixed_scale_into(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        scale: P::Scalar,
        output: &mut FloatFlatContoursBuffer<P>,
    ) -> Result<(), FixedScaleOverlayError> {
        self.stroke_custom_fixed_scale_into(style, is_closed_path, Default::default(), scale, output)
    }

    fn stroke_custom(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        options: OverlayOptions<P::Scalar>,
    ) -> Shapes<P> {
        match StrokeSolver::<P, i32>::prepare(self, style) {
            Some(solver) => solver.build(self, is_closed_path, options),
            None => vec![],
        }
    }

    fn stroke_custom_into(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        options: OverlayOptions<P::Scalar>,
        output: &mut FloatFlatContoursBuffer<P>,
    ) {
        match StrokeSolver::<P, i32>::prepare(self, style) {
            Some(solver) => solver.build_into(self, is_closed_path, options, output),
            None => output.clear_and_reserve(0, 0),
        }
    }

    fn stroke_custom_fixed_scale(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        options: OverlayOptions<P::Scalar>,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError> {
        let mut solver = match StrokeSolver::<P, i32>::prepare(self, style) {
            Some(solver) => solver,
            None => return Ok(vec![]),
        };
        solver.apply_scale(scale)?;
        Ok(solver.build(self, is_closed_path, options))
    }

    fn stroke_custom_fixed_scale_into(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        options: OverlayOptions<P::Scalar>,
        scale: P::Scalar,
        output: &mut FloatFlatContoursBuffer<P>,
    ) -> Result<(), FixedScaleOverlayError> {
        let mut solver = match StrokeSolver::<P, i32>::prepare(self, style) {
            Some(solver) => solver,
            None => {
                output.clear_and_reserve(0, 0);
                return Ok(());
            }
        };
        solver.apply_scale(scale)?;
        solver.build_into(self, is_closed_path, options, output);
        Ok(())
    }

    fn stroke_as<I>(&self, style: StrokeStyle<P>, is_closed_path: bool) -> Shapes<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static,
    {
        self.stroke_custom_as::<I>(style, is_closed_path, Default::default())
    }

    fn stroke_into_as<I>(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        output: &mut FloatFlatContoursBuffer<P>,
    ) where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static,
    {
        self.stroke_custom_into_as::<I>(style, is_closed_path, Default::default(), output)
    }

    fn stroke_custom_as<I>(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        options: OverlayOptions<P::Scalar, I>,
    ) -> Shapes<P>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static,
    {
        match StrokeSolver::<P, I>::prepare(self, style) {
            Some(solver) => solver.build(self, is_closed_path, options),
            None => vec![],
        }
    }

    fn stroke_custom_into_as<I>(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        options: OverlayOptions<P::Scalar, I>,
        output: &mut FloatFlatContoursBuffer<P>,
    ) where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static,
    {
        match StrokeSolver::<P, I>::prepare(self, style) {
            Some(solver) => solver.build_into(self, is_closed_path, options, output),
            None => output.clear_and_reserve(0, 0),
        }
    }

    fn stroke_fixed_scale_as<I>(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static,
    {
        self.stroke_custom_fixed_scale_as::<I>(style, is_closed_path, Default::default(), scale)
    }

    fn stroke_fixed_scale_into_as<I>(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        scale: P::Scalar,
        output: &mut FloatFlatContoursBuffer<P>,
    ) -> Result<(), FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static,
    {
        self.stroke_custom_fixed_scale_into_as::<I>(style, is_closed_path, Default::default(), scale, output)
    }

    fn stroke_custom_fixed_scale_as<I>(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        options: OverlayOptions<P::Scalar, I>,
        scale: P::Scalar,
    ) -> Result<Shapes<P>, FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static,
    {
        let mut solver = match StrokeSolver::<P, I>::prepare(self, style) {
            Some(solver) => solver,
            None => return Ok(vec![]),
        };
        solver.apply_scale(scale)?;
        Ok(solver.build(self, is_closed_path, options))
    }

    fn stroke_custom_fixed_scale_into_as<I>(
        &self,
        style: StrokeStyle<P>,
        is_closed_path: bool,
        options: OverlayOptions<P::Scalar, I>,
        scale: P::Scalar,
        output: &mut FloatFlatContoursBuffer<P>,
    ) -> Result<(), FixedScaleOverlayError>
    where
        I: IntNumber + Expiration + LayoutNumber + SortKey + 'static,
    {
        let mut solver = match StrokeSolver::<P, I>::prepare(self, style) {
            Some(solver) => solver,
            None => {
                output.clear_and_reserve(0, 0);
                return Ok(());
            }
        };
        solver.apply_scale(scale)?;
        solver.build_into(self, is_closed_path, options, output);
        Ok(())
    }
}

struct StrokeSolver<P: FloatPointCompatible, I: IntNumber> {
    r: P::Scalar,
    builder: StrokeBuilder<P, I>,
    adapter: FloatPointAdapter<P, I>,
    paths_count: usize,
    points_count: usize,
}

impl<P, I> StrokeSolver<P, I>
where
    P: 'static + FloatPointCompatible,
    I: 'static + IntNumber + Expiration + LayoutNumber + SortKey,
{
    fn prepare<S: ShapeResource<P>>(source: &S, style: StrokeStyle<P>) -> Option<Self> {
        let mut paths_count = 0;
        let mut points_count = 0;
        for path in source.iter_paths() {
            paths_count += 1;
            points_count += path.len();
        }

        if paths_count == 0 {
            return None;
        }

        let r = P::Scalar::from_float(0.5 * style.width.to_f64());
        let builder = StrokeBuilder::<P, I>::new(style);
        let a = builder.additional_offset(r);

        let mut rect = FloatRect::with_iter(source.iter_paths().flatten()).unwrap_or(FloatRect::zero());
        rect.add_offset(a);
        let adapter = FloatPointAdapter::<P, I>::new(rect);

        Some(Self {
            r,
            builder,
            adapter,
            paths_count,
            points_count,
        })
    }

    fn apply_scale(&mut self, scale: P::Scalar) -> Result<(), FixedScaleOverlayError> {
        self.adapter = FloatPointAdapter::try_with_scale(*self.adapter.rect(), scale)?;
        Ok(())
    }

    fn build<S: ShapeResource<P>>(
        self,
        source: &S,
        is_closed_path: bool,
        options: OverlayOptions<P::Scalar, I>,
    ) -> Shapes<P> {
        let ir = self.adapter.round_len_to_int(self.r).wide().unsigned_abs();
        if ir <= I::WideUInt::ONE {
            // offset is too small
            return vec![];
        }

        let capacity = self
            .builder
            .capacity(self.paths_count, self.points_count, is_closed_path);
        let mut segments = Vec::with_capacity(capacity);

        for path in source.iter_paths() {
            self.builder
                .build(path, is_closed_path, &self.adapter, &mut segments);
        }

        let mut overlay = Overlay::with_segments(segments);
        overlay.options = options.int_with_adapter(&self.adapter);

        let shapes = overlay.overlay(OverlayRule::Subject, FillRule::Positive);

        let mut float = shapes.to_float(&self.adapter);

        if options.clean_result {
            if options.preserve_output_collinear {
                float.despike_contour(&self.adapter);
            } else {
                float.simplify_contour(&self.adapter);
            }
        };

        float
    }

    fn build_into<S: ShapeResource<P>>(
        self,
        source: &S,
        is_closed_path: bool,
        options: OverlayOptions<P::Scalar, I>,
        output: &mut FloatFlatContoursBuffer<P>,
    ) {
        let ir = self.adapter.round_len_to_int(self.r).wide().unsigned_abs();
        if ir <= I::WideUInt::ONE {
            // offset is too small
            output.clear_and_reserve(0, 0);
            return;
        }

        let capacity = self
            .builder
            .capacity(self.paths_count, self.points_count, is_closed_path);
        let mut segments = Vec::with_capacity(capacity);

        for path in source.iter_paths() {
            self.builder
                .build(path, is_closed_path, &self.adapter, &mut segments);
        }

        let mut overlay = Overlay::with_segments(segments);
        overlay.options = options.int_with_adapter(&self.adapter);

        let mut int_output = FlatContoursBuffer::<I>::with_capacity(0);
        overlay.overlay_into(OverlayRule::Subject, FillRule::Positive, &mut int_output);

        let iter = int_output.points.iter().map(|p| self.adapter.int_to_float(p));
        output.set_with_iter(iter, &int_output.ranges);

        if options.clean_result {
            if options.preserve_output_collinear {
                output.despike_contour(&self.adapter);
            } else {
                output.simplify_contour(&self.adapter);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::mesh::stroke::offset::StrokeOffset;
    use crate::mesh::style::{LineCap, LineJoin, StrokeStyle};
    use alloc::vec;
    use alloc::vec::Vec;
    use core::f32::consts::PI;
    use i_shape::flat::float::FloatFlatContoursBuffer;

    #[test]
    fn test_doc() {
        let path = [
            [2.0, 1.0],
            [5.0, 1.0],
            [8.0, 4.0],
            [11.0, 4.0],
            [11.0, 1.0],
            [8.0, 1.0],
            [5.0, 4.0],
            [2.0, 4.0],
        ];

        let style = StrokeStyle::new(1.0)
            .line_join(LineJoin::Miter(1.0))
            .start_cap(LineCap::Round(0.1))
            .end_cap(LineCap::Square);

        let shapes = path.stroke(style, false);

        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 2);
    }

    #[test]
    fn test_simple() {
        let path = [[0.0, 0.0], [10.0, 0.0]];

        let style = StrokeStyle::new(2.0);
        let shapes = path.stroke(style, false);

        assert_eq!(shapes.len(), 1);
    }

    #[test]
    fn test_simple_as() {
        let path = [[0.0, 0.0], [10.0, 0.0]];

        let style = StrokeStyle::new(2.0);
        let shapes = path.stroke_as::<i64>(style, false);

        assert_eq!(shapes.len(), 1);
    }

    #[test]
    fn test_bevel_join() {
        let path = [[-10.0, 0.0], [0.0, 0.0], [0.0, 10.0]];

        let style = StrokeStyle::new(2.0);
        let shapes = path.stroke(style, false);

        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 1);

        let path = shape.first().unwrap();
        assert_eq!(path.len(), 7);
    }

    #[test]
    fn test_round_join() {
        let path = [[-10.0, 0.0], [0.0, 0.0], [0.0, 10.0]];

        let style = StrokeStyle::new(2.0).line_join(LineJoin::Round(0.25 * PI));
        let shapes = path.stroke(style, false);

        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 1);
    }

    #[test]
    fn test_miter_join_turn_right() {
        let path = [[-6.0, -12.0], [0.0, 0.0], [6.0, -12.0]];

        let style = StrokeStyle::new(2.0).line_join(LineJoin::Miter(5.0 * PI / 180.0));
        let shapes = path.stroke(style, false);

        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 1);
    }

    #[test]
    fn test_simple_closed() {
        let path = [[-5.0, -5.0], [-5.0, 5.0], [5.0, 5.0], [5.0, -5.0]];

        let style = StrokeStyle::new(2.0);

        let shapes = path.stroke_custom(style, true, Default::default());
        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 2);

        assert_eq!(shape[0].len(), 8);
        assert_eq!(shape[1].len(), 4);
    }

    #[test]
    fn test_miter_0() {
        let path = [
            [550.0, 225.0],
            [500.0, 250.0],
            [450.0, 275.0],
            [500.0, 300.0],
            [550.0, 325.0],
        ];

        let style = StrokeStyle::new(10.0).line_join(LineJoin::Miter(0.1));

        let shapes = path.stroke_custom(style, false, Default::default());
        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 1);
    }

    #[test]
    fn test_miter_1() {
        let path = [[100.0, 100.0], [200.0, 200.0], [150.0, 250.0]];

        let style = StrokeStyle::new(10.0).line_join(LineJoin::Miter(0.1));

        let shapes = path.stroke_custom(style, false, Default::default());
        assert_eq!(shapes.len(), 1);

        let shape = shapes.first().unwrap();
        assert_eq!(shape.len(), 1);
    }

    #[test]
    fn test_degenerate_0() {
        let path: Vec<[f64; 2]> = Vec::new();

        let style = StrokeStyle::new(2.0);
        let shapes = path.stroke(style, false);

        assert_eq!(shapes.len(), 0);
    }

    #[test]
    fn test_degenerate_1() {
        let path = [[0.0, 0.0]];

        let style = StrokeStyle::new(2.0);
        let shapes = path.stroke(style, false);

        assert_eq!(shapes.len(), 0);
    }

    #[test]
    fn test_degenerate_2() {
        let path = [[0.0, 0.0]];

        let style = StrokeStyle::new(2.0).end_cap(LineCap::Round(0.25 * PI));
        let shapes = path.stroke(style, false);

        assert_eq!(shapes.len(), 0);
    }

    #[test]
    fn test_degenerate_3() {
        let path = [[0.0, 0.0]];

        let style = StrokeStyle::new(2.0)
            .start_cap(LineCap::Butt)
            .end_cap(LineCap::Round(0.25 * PI));
        let shapes = path.stroke(style, false);

        assert_eq!(shapes.len(), 0);
    }

    #[test]
    fn test_many_paths() {
        let paths = [vec![[0.0, 0.0], [5.0, 0.0]], vec![[0.0, 0.0], [5.0, -5.0]]];

        let style = StrokeStyle::new(2.0)
            .start_cap(LineCap::Butt)
            .end_cap(LineCap::Butt);
        let shapes = paths.stroke(style, false);

        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].len(), 1);
        assert_eq!(shapes[0][0].len(), 8);
    }

    #[test]
    fn test_stroke_fixed_scale_ok() {
        let path = [[0.0, 0.0], [10.0, 0.0]];
        let style = StrokeStyle::new(2.0);

        let shapes = path.stroke_fixed_scale(style, false, 10.0).unwrap();

        assert_eq!(shapes.len(), 1);
    }

    #[test]
    fn test_stroke_into_ok() {
        let path = [[0.0, 0.0], [10.0, 0.0]];
        let style = StrokeStyle::new(2.0);
        let mut output = FloatFlatContoursBuffer::default();

        path.stroke_into(style, false, &mut output);

        assert!(!output.is_empty());
        assert!(!output.ranges.is_empty());
    }

    #[test]
    fn test_stroke_fixed_scale_into_ok() {
        let path = [[0.0, 0.0], [10.0, 0.0]];
        let style = StrokeStyle::new(2.0);
        let mut output = FloatFlatContoursBuffer::default();

        path.stroke_fixed_scale_into(style, false, 10.0, &mut output)
            .unwrap();

        assert!(!output.is_empty());
        assert!(!output.ranges.is_empty());
    }

    #[test]
    fn test_stroke_fixed_scale_invalid() {
        let path = [[0.0, 0.0], [10.0, 0.0]];
        let style = StrokeStyle::new(2.0);

        assert!(path.stroke_fixed_scale(style.clone(), false, 0.0).is_err());
        assert!(path.stroke_fixed_scale(style.clone(), false, -1.0).is_err());
        assert!(path.stroke_fixed_scale(style.clone(), false, f64::NAN).is_err());
        assert!(
            path.stroke_fixed_scale(style.clone(), false, f64::INFINITY)
                .is_err()
        );
    }
}
