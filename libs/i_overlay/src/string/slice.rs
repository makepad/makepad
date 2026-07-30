use crate::core::fill_rule::FillRule;
use crate::string::line::IntLine;
use crate::string::overlay::StringOverlay;
use crate::string::rule::StringRule;
use i_float::int::number::int::IntNumber;
use i_float::int::point::IntPoint;
use i_key_sort::sort::key::SortKey;
use i_shape::int::path::IntPath;
use i_shape::int::shape::{IntShape, IntShapes};
use i_tree::{Expiration, LayoutNumber};

pub trait IntSlice<I: IntNumber> {
    fn slice_by_line(&self, line: IntLine<I>, fill_rule: FillRule) -> IntShapes<I>;
    fn slice_by_lines(&self, lines: &[IntLine<I>], fill_rule: FillRule) -> IntShapes<I>;
    fn slice_by_path(&self, path: &IntPath<I>, fill_rule: FillRule) -> IntShapes<I>;
    fn slice_by_paths(&self, paths: &[IntPath<I>], fill_rule: FillRule) -> IntShapes<I>;
}

impl<I> IntSlice<I> for IntShapes<I>
where
    I: IntNumber + Expiration + LayoutNumber + SortKey,
{
    #[inline]
    fn slice_by_line(&self, line: IntLine<I>, fill_rule: FillRule) -> IntShapes<I> {
        let mut overlay = StringOverlay::with_shapes(self);
        overlay.add_string_line(line);
        overlay
            .build_graph_view(fill_rule)
            .map(|graph| graph.extract_shapes(StringRule::Slice))
            .unwrap_or_default()
    }

    #[inline]
    fn slice_by_lines(&self, lines: &[IntLine<I>], fill_rule: FillRule) -> IntShapes<I> {
        let mut overlay = StringOverlay::with_shapes(self);
        overlay.add_string_lines(lines);
        overlay
            .build_graph_view(fill_rule)
            .map(|graph| graph.extract_shapes(StringRule::Slice))
            .unwrap_or_default()
    }

    #[inline]
    fn slice_by_path(&self, path: &IntPath<I>, fill_rule: FillRule) -> IntShapes<I> {
        let mut overlay = StringOverlay::with_shapes(self);
        overlay.add_string_path(path);
        overlay
            .build_graph_view(fill_rule)
            .map(|graph| graph.extract_shapes(StringRule::Slice))
            .unwrap_or_default()
    }

    #[inline]
    fn slice_by_paths(&self, paths: &[IntPath<I>], fill_rule: FillRule) -> IntShapes<I> {
        let mut overlay = StringOverlay::with_shapes(self);
        overlay.add_string_paths(paths);
        overlay
            .build_graph_view(fill_rule)
            .map(|graph| graph.extract_shapes(StringRule::Slice))
            .unwrap_or_default()
    }
}

impl<I> IntSlice<I> for IntShape<I>
where
    I: IntNumber + Expiration + LayoutNumber + SortKey,
{
    #[inline]
    fn slice_by_line(&self, line: IntLine<I>, fill_rule: FillRule) -> IntShapes<I> {
        let mut overlay = StringOverlay::with_shape(self);
        overlay.add_string_line(line);
        overlay
            .build_graph_view(fill_rule)
            .map(|graph| graph.extract_shapes(StringRule::Slice))
            .unwrap_or_default()
    }

    #[inline]
    fn slice_by_lines(&self, lines: &[IntLine<I>], fill_rule: FillRule) -> IntShapes<I> {
        let mut overlay = StringOverlay::with_shape(self);
        overlay.add_string_lines(lines);
        overlay
            .build_graph_view(fill_rule)
            .map(|graph| graph.extract_shapes(StringRule::Slice))
            .unwrap_or_default()
    }

    #[inline]
    fn slice_by_path(&self, path: &IntPath<I>, fill_rule: FillRule) -> IntShapes<I> {
        let mut overlay = StringOverlay::with_shape(self);
        overlay.add_string_path(path);
        overlay
            .build_graph_view(fill_rule)
            .map(|graph| graph.extract_shapes(StringRule::Slice))
            .unwrap_or_default()
    }

    #[inline]
    fn slice_by_paths(&self, paths: &[IntPath<I>], fill_rule: FillRule) -> IntShapes<I> {
        let mut overlay = StringOverlay::with_shape(self);
        overlay.add_string_paths(paths);
        overlay
            .build_graph_view(fill_rule)
            .map(|graph| graph.extract_shapes(StringRule::Slice))
            .unwrap_or_default()
    }
}

impl<I> IntSlice<I> for [IntPoint<I>]
where
    I: IntNumber + Expiration + LayoutNumber + SortKey,
{
    #[inline]
    fn slice_by_line(&self, line: IntLine<I>, fill_rule: FillRule) -> IntShapes<I> {
        let mut overlay = StringOverlay::with_shape_contour(self);
        overlay.add_string_line(line);
        overlay
            .build_graph_view(fill_rule)
            .map(|graph| graph.extract_shapes(StringRule::Slice))
            .unwrap_or_default()
    }

    #[inline]
    fn slice_by_lines(&self, lines: &[IntLine<I>], fill_rule: FillRule) -> IntShapes<I> {
        let mut overlay = StringOverlay::with_shape_contour(self);
        overlay.add_string_lines(lines);
        overlay
            .build_graph_view(fill_rule)
            .map(|graph| graph.extract_shapes(StringRule::Slice))
            .unwrap_or_default()
    }

    #[inline]
    fn slice_by_path(&self, path: &IntPath<I>, fill_rule: FillRule) -> IntShapes<I> {
        let mut overlay = StringOverlay::with_shape_contour(self);
        overlay.add_string_path(path);
        overlay
            .build_graph_view(fill_rule)
            .map(|graph| graph.extract_shapes(StringRule::Slice))
            .unwrap_or_default()
    }

    #[inline]
    fn slice_by_paths(&self, paths: &[IntPath<I>], fill_rule: FillRule) -> IntShapes<I> {
        let mut overlay = StringOverlay::with_shape_contour(self);
        overlay.add_string_paths(paths);
        overlay
            .build_graph_view(fill_rule)
            .map(|graph| graph.extract_shapes(StringRule::Slice))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::useless_vec)]

    use crate::core::fill_rule::FillRule;
    use crate::string::slice::IntSlice;
    use alloc::vec;
    use i_float::int::point::IntPoint;

    #[test]
    fn test_empty_input() {
        #[rustfmt::skip]
        let shapes = [].slice_by_line(
            [IntPoint::new(0, 0), IntPoint::new(0, 0)],
            FillRule::NonZero,
        );

        assert_eq!(shapes.len(), 0);
    }

    #[test]
    fn test_0() {
        let paths = vec![vec![
            IntPoint::new(-10, 10),
            IntPoint::new(-10, -10),
            IntPoint::new(10, -10),
            IntPoint::new(10, 10),
        ]];

        let result = paths.slice_by_line([IntPoint::new(-20, 0), IntPoint::new(20, 0)], FillRule::NonZero);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].len(), 1);
        assert_eq!(result[0][0].len(), 4);
        assert_eq!(result[1].len(), 1);
        assert_eq!(result[1][0].len(), 4);
    }

    #[test]
    fn test_i64_slice() {
        let paths = vec![vec![
            IntPoint::<i64>::new(-10, 10),
            IntPoint::new(-10, -10),
            IntPoint::new(10, -10),
            IntPoint::new(10, 10),
        ]];

        let result = paths.slice_by_line([IntPoint::new(-20, 0), IntPoint::new(20, 0)], FillRule::NonZero);

        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_1() {
        let paths = vec![
            vec![
                IntPoint::new(-10, 10),
                IntPoint::new(-10, -10),
                IntPoint::new(10, -10),
                IntPoint::new(10, 10),
            ],
            vec![
                IntPoint::new(-5, -5),
                IntPoint::new(-5, 5),
                IntPoint::new(5, 5),
                IntPoint::new(5, -5),
            ],
        ];

        let result = paths.slice_by_line([IntPoint::new(-20, 0), IntPoint::new(20, 0)], FillRule::NonZero);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].len(), 1);
        assert_eq!(result[0][0].len(), 8);
        assert_eq!(result[1].len(), 1);
        assert_eq!(result[1][0].len(), 8);
    }

    #[test]
    fn test_2() {
        let paths = vec![
            vec![
                IntPoint::new(0, 0),
                IntPoint::new(35, 0),
                IntPoint::new(35, 20),
                IntPoint::new(0, 20),
            ],
            vec![
                IntPoint::new(5, 5),
                IntPoint::new(5, 15),
                IntPoint::new(15, 15),
                IntPoint::new(15, 5),
            ],
            vec![
                IntPoint::new(20, 5),
                IntPoint::new(20, 15),
                IntPoint::new(30, 15),
                IntPoint::new(30, 5),
            ],
        ];

        let result = paths.slice_by_line([IntPoint::new(15, 10), IntPoint::new(20, 10)], FillRule::NonZero);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].len(), 3);
    }

    #[test]
    fn test_3() {
        let paths = vec![
            vec![
                IntPoint::new(0, 0),
                IntPoint::new(35, 0),
                IntPoint::new(35, 20),
                IntPoint::new(0, 20),
            ],
            vec![
                IntPoint::new(5, 5),
                IntPoint::new(5, 15),
                IntPoint::new(15, 15),
                IntPoint::new(15, 5),
            ],
            vec![
                IntPoint::new(20, 5),
                IntPoint::new(20, 15),
                IntPoint::new(30, 15),
                IntPoint::new(30, 5),
            ],
        ];

        let result = paths.slice_by_lines(
            &vec![
                [IntPoint::new(15, 5), IntPoint::new(20, 5)],
                [IntPoint::new(15, 15), IntPoint::new(20, 15)],
            ],
            FillRule::NonZero,
        );

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].len(), 2);
        assert_eq!(result[1].len(), 1);
    }

    #[test]
    fn test_4() {
        let paths = vec![
            vec![
                IntPoint::new(0, 0),
                IntPoint::new(35, 0),
                IntPoint::new(35, 35),
                IntPoint::new(0, 35),
            ],
            vec![
                IntPoint::new(5, 5),
                IntPoint::new(5, 15),
                IntPoint::new(15, 15),
                IntPoint::new(15, 5),
            ],
            vec![
                IntPoint::new(20, 5),
                IntPoint::new(20, 15),
                IntPoint::new(30, 15),
                IntPoint::new(30, 5),
            ],
            vec![
                IntPoint::new(5, 20),
                IntPoint::new(5, 30),
                IntPoint::new(15, 30),
                IntPoint::new(15, 20),
            ],
            vec![
                IntPoint::new(20, 20),
                IntPoint::new(20, 30),
                IntPoint::new(30, 30),
                IntPoint::new(30, 20),
            ],
        ];

        let result = paths.slice_by_lines(
            &vec![
                [IntPoint::new(10, 15), IntPoint::new(10, 20)],
                [IntPoint::new(25, 15), IntPoint::new(25, 20)],
                [IntPoint::new(15, 10), IntPoint::new(20, 10)],
                [IntPoint::new(15, 25), IntPoint::new(20, 25)],
            ],
            FillRule::NonZero,
        );

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].len(), 2);
        assert_eq!(result[1].len(), 1);
    }

    #[test]
    fn test_5() {
        #[rustfmt::skip]
        let shapes = vec![
            IntPoint::new(-2, -2),
            IntPoint::new(2, -2),
            IntPoint::new(2, 2),
            IntPoint::new(-2, 2),
        ]
        .slice_by_line(
            [IntPoint::new(-5, 5), IntPoint::new(5, -5)],
            FillRule::NonZero,
        );

        assert_eq!(shapes.len(), 2);
        assert_eq!(shapes[0][0].len(), 3);
        assert_eq!(shapes[1][0].len(), 3);
    }

    #[test]
    fn test_6() {
        #[rustfmt::skip]
        let shapes = vec![
            IntPoint::new(-2, -2),
            IntPoint::new(2, -2),
            IntPoint::new(2, 2),
            IntPoint::new(-2, 2),
        ]
        .slice_by_line(
            [IntPoint::new(-5, 5), IntPoint::new(5, 5)],
            FillRule::NonZero,
        );

        assert_eq!(shapes.len(), 1);
    }

    #[test]
    fn test_7() {
        #[rustfmt::skip]
        let shapes = vec![
            IntPoint::new(0, 0),
            IntPoint::new(2, 0),
            IntPoint::new(2, 3),
            IntPoint::new(1, 1),
            IntPoint::new(0, 3),
        ]
        .slice_by_line(
            [IntPoint::new(-5, 2), IntPoint::new(5, 2)],
            FillRule::NonZero,
        );

        assert_eq!(shapes.len(), 3);
        assert_eq!(shapes[0][0].len(), 5);
        assert_eq!(shapes[1][0].len(), 3);
        assert_eq!(shapes[2][0].len(), 3);
    }

    #[test]
    fn test_8() {
        #[rustfmt::skip]
        let shapes = vec![
            IntPoint::new(-2, -2),
            IntPoint::new(2, -2),
            IntPoint::new(2, 2),
            IntPoint::new(-2, 2),
        ]
        .slice_by_line(
            [IntPoint::new(-2, 5), IntPoint::new(-2, -5)],
            FillRule::NonZero,
        );

        assert_eq!(shapes.len(), 1);
    }

    #[test]
    fn test_9() {
        #[rustfmt::skip]
        let shapes = vec![
            IntPoint::new(-2, 0),
            IntPoint::new(2, 0),
            IntPoint::new(0, 2),
        ]
        .slice_by_line(
            [IntPoint::new(-5, 2), IntPoint::new(5, 2)],
            FillRule::NonZero,
        );

        assert_eq!(shapes.len(), 1);
    }
}
