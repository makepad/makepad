use crate::int::count::PointsCount;
use crate::int::shape::{IntContour, IntShape};
use crate::util::reserve::Reserve;
use alloc::vec::Vec;
use core::ops::Range;
use i_float::int::number::int::IntNumber;
use i_float::int::point::IntPoint;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub struct FlatContoursBuffer<T: IntNumber> {
    pub points: Vec<IntPoint<T>>,
    pub ranges: Vec<Range<usize>>,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub struct FlatShapesBuffer<T: IntNumber> {
    pub points: Vec<IntPoint<T>>,
    pub contour_ranges: Vec<Range<usize>>,
    pub shape_ranges: Vec<Range<usize>>,
}

impl<T: IntNumber> Default for FlatContoursBuffer<T> {
    fn default() -> Self {
        Self {
            points: Vec::new(),
            ranges: Vec::new(),
        }
    }
}

impl<T: IntNumber> Default for FlatShapesBuffer<T> {
    fn default() -> Self {
        Self {
            points: Vec::new(),
            contour_ranges: Vec::new(),
            shape_ranges: Vec::new(),
        }
    }
}

impl<T: IntNumber> FlatContoursBuffer<T> {
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            points: Vec::with_capacity(capacity),
            ranges: Vec::new(),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    #[inline]
    pub fn is_single_contour(&self) -> bool {
        self.ranges.len() == 1
    }

    #[inline]
    pub fn as_first_contour(&self) -> &[IntPoint<T>] {
        if let Some(first_contour_range) = self.ranges.first() {
            &self.points[first_contour_range.clone()]
        } else {
            &self.points
        }
    }

    #[inline]
    pub fn as_first_contour_mut(&mut self) -> &mut [IntPoint<T>] {
        if let Some(first_contour_range) = self.ranges.first() {
            &mut self.points[first_contour_range.clone()]
        } else {
            &mut self.points
        }
    }

    #[inline]
    pub fn set_with_contour(&mut self, contour: &[IntPoint<T>]) {
        let points_len = contour.len();
        self.clear_and_reserve(points_len, 1);

        self.points.extend_from_slice(contour);
        self.ranges.push(0..points_len);
    }

    #[inline]
    pub fn set_with_shape(&mut self, shape: &[IntContour<T>]) {
        let points_len = shape.points_count();
        let contours_len = shape.len();
        self.clear_and_reserve(points_len, contours_len);

        let mut offset = 0;
        for contour in shape.iter() {
            let len = contour.len();
            self.points.extend_from_slice(contour);
            self.ranges.push(offset..offset + len);
            offset += len;
        }
    }

    #[inline]
    pub fn set_with_shapes(&mut self, shapes: &[IntShape<T>]) {
        let points_len = shapes.points_count();
        let contours_len = shapes.iter().map(Vec::len).sum();
        self.clear_and_reserve(points_len, contours_len);

        let mut points_offset = 0;
        for shape in shapes.iter() {
            for contour in shape.iter() {
                let len = contour.len();
                self.points.extend_from_slice(contour);
                self.ranges.push(points_offset..points_offset + len);
                points_offset += len;
            }
        }
    }

    #[inline]
    pub fn clear_and_reserve(&mut self, points: usize, contours: usize) {
        self.points.reserve_capacity(points);
        self.points.clear();

        self.ranges.reserve_capacity(contours);
        self.ranges.clear();
    }

    #[inline]
    pub fn add_contour(&mut self, contour: &[IntPoint<T>]) {
        let start = self.points.len();
        let end = start + contour.len();
        self.ranges.push(start..end);
        self.points.extend_from_slice(contour);
    }

    #[inline]
    pub fn to_contours(&self) -> Vec<IntContour<T>> {
        let mut contours = Vec::with_capacity(self.ranges.len());

        for range in self.ranges.iter() {
            contours.push(self.points[range.clone()].to_vec());
        }

        contours
    }
}

impl<T: IntNumber> FlatShapesBuffer<T> {
    #[inline]
    pub fn with_capacity(points: usize, contours: usize, shapes: usize) -> Self {
        Self {
            points: Vec::with_capacity(points),
            contour_ranges: Vec::with_capacity(contours),
            shape_ranges: Vec::with_capacity(shapes),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    #[inline]
    pub fn set_with_contour(&mut self, contour: &[IntPoint<T>]) {
        let points_len = contour.len();
        self.clear_and_reserve(points_len, 1, 1);

        self.points.extend_from_slice(contour);
        self.contour_ranges.push(0..points_len);
        self.shape_ranges.push(0..1);
    }

    #[inline]
    pub fn set_with_shape(&mut self, shape: &[IntContour<T>]) {
        let points_len = shape.points_count();
        let contours_len = shape.len();
        self.clear_and_reserve(points_len, contours_len, 1);

        let mut points_offset = 0;
        for contour in shape.iter() {
            let len = contour.len();
            self.points.extend_from_slice(contour);
            self.contour_ranges.push(points_offset..points_offset + len);
            points_offset += len;
        }

        self.shape_ranges.push(0..contours_len);
    }

    #[inline]
    pub fn set_with_shapes(&mut self, shapes: &[IntShape<T>]) {
        let points_len = shapes.points_count();
        let contours_len = shapes.iter().map(Vec::len).sum();
        let shapes_len = shapes.len();
        self.clear_and_reserve(points_len, contours_len, shapes_len);

        let mut points_offset = 0;
        let mut contours_offset = 0;
        for shape in shapes.iter() {
            let shape_start = contours_offset;
            for contour in shape.iter() {
                let len = contour.len();
                self.points.extend_from_slice(contour);
                self.contour_ranges.push(points_offset..points_offset + len);
                points_offset += len;
                contours_offset += 1;
            }
            self.shape_ranges.push(shape_start..contours_offset);
        }
    }

    #[inline]
    pub fn clear_and_reserve(&mut self, points: usize, contours: usize, shapes: usize) {
        self.points.reserve_capacity(points);
        self.points.clear();

        self.contour_ranges.reserve_capacity(contours);
        self.contour_ranges.clear();

        self.shape_ranges.reserve_capacity(shapes);
        self.shape_ranges.clear();
    }

    #[inline]
    pub fn add_shape(&mut self, shape: &[IntContour<T>]) {
        let shape_start = self.contour_ranges.len();
        let mut points_offset = self.points.len();
        for contour in shape.iter() {
            let len = contour.len();
            self.points.extend_from_slice(contour);
            self.contour_ranges.push(points_offset..points_offset + len);
            points_offset += len;
        }
        self.shape_ranges.push(shape_start..self.contour_ranges.len());
    }

    #[inline]
    pub fn to_shapes(&self) -> Vec<IntShape<T>> {
        let mut shapes = Vec::with_capacity(self.shape_ranges.len());

        for shape_range in self.shape_ranges.iter() {
            let mut shape = Vec::with_capacity(shape_range.len());
            for contour_index in shape_range.clone() {
                let contour_range = self.contour_ranges[contour_index].clone();
                shape.push(self.points[contour_range].to_vec());
            }
            shapes.push(shape);
        }

        shapes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::int::shape::{IntContour, IntShape, IntShapes};
    use alloc::vec;
    use rand::RngExt;

    fn make_contour(p: &[(i32, i32)]) -> IntContour<i32> {
        p.iter().map(|&(x, y)| IntPoint::new(x, y)).collect()
    }

    #[test]
    fn test_contour_flat_round_trip() {
        let contour = make_contour(&[(1, 2), (3, 4), (5, 6)]);
        let mut flat = FlatContoursBuffer::with_capacity(0);
        flat.set_with_contour(&contour);

        let contours = flat.to_contours();
        assert_eq!(contours.len(), 1);
        assert_eq!(contours[0], contour);
    }

    #[test]
    fn test_shape_flat_round_trip() {
        let shape = vec![
            make_contour(&[(0, 0), (1, 0), (1, 1), (0, 1)]),
            make_contour(&[(2, 2), (3, 2), (3, 3), (2, 3)]),
        ];
        let mut flat = FlatContoursBuffer::with_capacity(0);
        flat.set_with_shape(&shape);

        let contours = flat.to_contours();
        assert_eq!(contours.len(), 2);
        assert_eq!(contours[0].len(), 4);
        assert_eq!(contours[1].len(), 4);
    }

    #[test]
    fn test_shapes_flat_round_trip() {
        let shapes = vec![
            vec![make_contour(&[(0, 0), (1, 0), (1, 1)])],
            vec![
                make_contour(&[(5, 5), (6, 5), (6, 6)]),
                make_contour(&[(7, 7), (8, 7), (8, 8)]),
            ],
        ];
        let mut flat = FlatContoursBuffer::with_capacity(0);
        flat.set_with_shapes(&shapes);

        let contours = flat.to_contours();
        assert_eq!(contours.len(), 3);
        assert_eq!(contours[0].len(), 3);
        assert_eq!(contours[1].len(), 3);
        assert_eq!(contours[2].len(), 3);
    }

    #[test]
    fn test_random_shapes_round_trip() {
        let mut rng = rand::rng();
        let mut shapes: IntShapes<i32> = Vec::new();

        for _ in 0..5 {
            let mut shape: IntShape<i32> = Vec::new();
            let contour_count = rng.random_range(1..4);
            for _ in 0..contour_count {
                let mut contour: IntContour<i32> = Vec::new();
                let point_count = rng.random_range(3..7);
                for _ in 0..point_count {
                    let x = rng.random_range(-100..100);
                    let y = rng.random_range(-100..100);
                    contour.push(IntPoint::new(x, y));
                }
                shape.push(contour);
            }
            shapes.push(shape);
        }

        let mut flat = FlatContoursBuffer::with_capacity(0);
        flat.set_with_shapes(&shapes);
        let contours = flat.to_contours();
        assert_eq!(contours.len(), shapes.iter().fold(0, |s, shape| s + shape.len()));
    }

    #[test]
    fn test_shapes_buffer_with_contour_round_trip() {
        let contour = make_contour(&[(1, 2), (3, 4), (5, 6)]);
        let mut flat = FlatShapesBuffer::with_capacity(0, 0, 0);
        flat.set_with_contour(&contour);

        assert_eq!(flat.contour_ranges, vec![0..3]);
        assert_eq!(flat.shape_ranges, vec![0..1]);

        let shapes = flat.to_shapes();
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].len(), 1);
        assert_eq!(shapes[0][0], contour);
    }

    #[test]
    fn test_shapes_buffer_with_shape_round_trip() {
        let shape = vec![
            make_contour(&[(0, 0), (1, 0), (1, 1), (0, 1)]),
            make_contour(&[(2, 2), (3, 2), (3, 3), (2, 3)]),
        ];
        let mut flat = FlatShapesBuffer::with_capacity(0, 0, 0);
        flat.set_with_shape(&shape);

        assert_eq!(flat.shape_ranges, vec![0..2]);
        assert_eq!(flat.contour_ranges, vec![0..4, 4..8]);

        let shapes = flat.to_shapes();
        assert_eq!(shapes, vec![shape]);
    }

    #[test]
    fn test_shapes_buffer_with_shapes_round_trip() {
        let shapes = vec![
            vec![make_contour(&[(0, 0), (1, 0), (1, 1)])],
            vec![
                make_contour(&[(5, 5), (6, 5), (6, 6)]),
                make_contour(&[(7, 7), (8, 7), (8, 8)]),
            ],
        ];

        let mut flat = FlatShapesBuffer::with_capacity(0, 0, 0);
        flat.set_with_shapes(&shapes);

        assert_eq!(flat.shape_ranges, vec![0..1, 1..3]);
        assert_eq!(flat.contour_ranges, vec![0..3, 3..6, 6..9]);
        assert_eq!(flat.to_shapes(), shapes);
    }

    #[test]
    fn test_shapes_buffer_add_shape() {
        let mut flat = FlatShapesBuffer::with_capacity(0, 0, 0);
        let shape0 = vec![make_contour(&[(0, 0), (1, 0), (1, 1)])];
        let shape1 = vec![
            make_contour(&[(2, 2), (3, 2), (3, 3)]),
            make_contour(&[(4, 4), (5, 4), (5, 5)]),
        ];

        flat.add_shape(&shape0);
        flat.add_shape(&shape1);

        assert_eq!(flat.shape_ranges, vec![0..1, 1..3]);
        assert_eq!(flat.contour_ranges, vec![0..3, 3..6, 6..9]);
        assert_eq!(flat.to_shapes(), vec![shape0, shape1]);
    }
}
