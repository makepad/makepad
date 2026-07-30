use crate::base::data::{Contour, Shape};
use crate::source::resource::ShapeResource;
use alloc::vec::Vec;
use core::ops::Range;
use i_float::float::compatible::FloatPointCompatible;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default)]
pub struct FloatFlatContoursBuffer<P> {
    pub points: Vec<P>,
    pub ranges: Vec<Range<usize>>,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default)]
pub struct FloatFlatShapesBuffer<P> {
    pub points: Vec<P>,
    pub contour_ranges: Vec<Range<usize>>,
    pub shape_ranges: Vec<Range<usize>>,
}

impl<P> FloatFlatContoursBuffer<P> {
    #[inline]
    pub fn with_capacity(points: usize, contours: usize) -> Self {
        Self {
            points: Vec::with_capacity(points.saturating_mul(2)),
            ranges: Vec::with_capacity(contours),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    #[inline]
    pub fn clear_and_reserve(&mut self, points: usize, contours: usize) {
        self.points.clear();
        self.points.reserve(points);

        self.ranges.clear();
        self.ranges.reserve(contours);
    }

    #[inline]
    pub fn add_contour(&mut self, contour: &[P])
    where
        P: Clone,
    {
        let start = self.points.len();
        self.points.extend_from_slice(contour);

        self.ranges.push(start..self.points.len());
    }

    #[inline]
    pub fn add_contour_iter<I>(&mut self, contour_iter: I)
    where
        I: IntoIterator<Item = P>,
        P: Clone,
    {
        let start = self.points.len();
        let mut iter = contour_iter.into_iter();
        self.points.extend(&mut iter);
        self.ranges.push(start..self.points.len());
    }

    #[inline]
    pub fn set_with_resource<R>(&mut self, resource: &R)
    where
        R: ShapeResource<P> + ?Sized,
        P: FloatPointCompatible,
    {
        let mut contours_count = 0;
        let mut points_count = 0;
        for contour in resource.iter_paths() {
            contours_count += 1;
            points_count += contour.len();
        }

        self.clear_and_reserve(points_count, contours_count);
        for contour in resource.iter_paths() {
            self.add_contour(contour);
        }
    }

    #[inline]
    pub fn set_flat(&mut self, points: &[P], ranges: &[Range<usize>])
    where
        P: Clone,
    {
        self.clear_and_reserve(points.len(), ranges.len());
        self.points.extend_from_slice(points);
        self.ranges.extend_from_slice(ranges);
    }

    #[inline]
    pub fn set_with_iter<I>(&mut self, iter: I, ranges: &[Range<usize>])
    where
        I: IntoIterator<Item = P>,
    {
        let mut iter = iter.into_iter();
        let max_range_end = ranges.iter().map(|r| r.end).max().unwrap_or(0);
        let (min_points, max_points) = iter.size_hint();
        let points_capacity = max_points.unwrap_or(min_points).max(max_range_end);
        self.clear_and_reserve(points_capacity, ranges.len());
        self.points.extend(&mut iter);
        self.ranges.extend_from_slice(ranges);
    }

    #[inline]
    pub fn to_contours(&self) -> Vec<Contour<P>>
    where
        P: Clone,
    {
        let mut contours = Vec::with_capacity(self.ranges.len());

        for range in self.ranges.iter() {
            let slice = &self.points[range.clone()];
            contours.push(slice.to_vec());
        }

        contours
    }

    #[inline]
    pub(crate) fn contour_pairs_at(&self, index: usize) -> Option<&[P]> {
        let range = self.ranges.get(index)?;
        if range.start >= range.end || range.end > self.points.len() {
            return None;
        }
        Some(&self.points[range.clone()])
    }
}

impl<P> FloatFlatShapesBuffer<P> {
    #[inline]
    pub fn with_capacity(points: usize, contours: usize, shapes: usize) -> Self {
        Self {
            points: Vec::with_capacity(points.saturating_mul(2)),
            contour_ranges: Vec::with_capacity(contours),
            shape_ranges: Vec::with_capacity(shapes),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    #[inline]
    pub fn clear_and_reserve(&mut self, points: usize, contours: usize, shapes: usize) {
        self.points.clear();
        self.points.reserve(points);

        self.contour_ranges.clear();
        self.contour_ranges.reserve(contours);

        self.shape_ranges.clear();
        self.shape_ranges.reserve(shapes);
    }

    #[inline]
    pub fn add_contour(&mut self, contour: &[P])
    where
        P: Clone,
    {
        let contour_start = self.points.len();
        self.points.extend_from_slice(contour);
        self.contour_ranges.push(contour_start..self.points.len());

        let shape_start = self.contour_ranges.len() - 1;
        self.shape_ranges.push(shape_start..shape_start + 1);
    }

    #[inline]
    pub fn add_contour_iter<I>(&mut self, contour_iter: I)
    where
        I: IntoIterator<Item = P>,
        P: Clone,
    {
        let contour_start = self.points.len();
        let mut iter = contour_iter.into_iter();
        self.points.extend(&mut iter);
        self.contour_ranges.push(contour_start..self.points.len());

        let shape_start = self.contour_ranges.len() - 1;
        self.shape_ranges.push(shape_start..shape_start + 1);
    }

    #[inline]
    pub fn set_with_resource<R>(&mut self, resource: &R)
    where
        R: ShapeResource<P> + ?Sized,
        P: FloatPointCompatible,
    {
        let mut contours_count = 0;
        let mut points_count = 0;
        for contour in resource.iter_paths() {
            contours_count += 1;
            points_count += contour.len();
        }

        self.clear_and_reserve(points_count, contours_count, usize::from(contours_count > 0));
        let mut offset = 0;
        for contour in resource.iter_paths() {
            let contour_start = offset;
            let contour_end = contour_start + contour.len();
            self.points.extend_from_slice(contour);
            self.contour_ranges.push(contour_start..contour_end);
            offset = contour_end;
        }
        if contours_count > 0 {
            self.shape_ranges.push(0..contours_count);
        }
    }

    #[inline]
    pub fn set_flat(&mut self, points: &[P], contour_ranges: &[Range<usize>], shape_ranges: &[Range<usize>])
    where
        P: Clone,
    {
        self.clear_and_reserve(points.len(), contour_ranges.len(), shape_ranges.len());
        self.points.extend_from_slice(points);
        self.contour_ranges.extend_from_slice(contour_ranges);
        self.shape_ranges.extend_from_slice(shape_ranges);
    }

    #[inline]
    pub fn set_with_iter<I>(
        &mut self,
        iter: I,
        contour_ranges: &[Range<usize>],
        shape_ranges: &[Range<usize>],
    ) where
        I: IntoIterator<Item = P>,
    {
        let mut iter = iter.into_iter();
        let max_contour_end = contour_ranges.iter().map(|r| r.end).max().unwrap_or(0);
        let (min_points, max_points) = iter.size_hint();
        let points_capacity = max_points.unwrap_or(min_points).max(max_contour_end);

        self.clear_and_reserve(points_capacity, contour_ranges.len(), shape_ranges.len());
        self.points.extend(&mut iter);
        self.contour_ranges.extend_from_slice(contour_ranges);
        self.shape_ranges.extend_from_slice(shape_ranges);
    }

    #[inline]
    pub fn to_contours(&self) -> Vec<Contour<P>>
    where
        P: Clone,
    {
        let mut contours = Vec::with_capacity(self.contour_ranges.len());

        for range in self.contour_ranges.iter() {
            let slice = &self.points[range.clone()];
            contours.push(slice.to_vec());
        }

        contours
    }

    #[inline]
    pub fn to_shapes(&self) -> Vec<Shape<P>>
    where
        P: Clone,
    {
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

    #[inline]
    pub(crate) fn contour_pairs_at(&self, index: usize) -> Option<&[P]> {
        let range = self.contour_ranges.get(index)?;
        if range.start >= range.end || range.end > self.points.len() {
            return None;
        }
        Some(&self.points[range.clone()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    #[test]
    fn test_add_contour() {
        let mut buffer = FloatFlatContoursBuffer::<[f64; 2]>::default();
        let c0 = [[0.0, 0.0], [2.0, 0.0], [2.0, 2.0]];
        let c1 = [[10.0, 10.0], [11.0, 10.0], [11.0, 11.0]];

        buffer.add_contour(&c0);
        buffer.add_contour(&c1);

        assert_eq!(buffer.ranges.len(), 2);
        assert_eq!(buffer.ranges[0], 0..3);
        assert_eq!(buffer.ranges[1], 3..6);
        assert_eq!(buffer.points.len(), 6);
    }

    #[test]
    fn test_set_with_resource() {
        let shape: Vec<Vec<[f64; 2]>> = vec![
            vec![[0.0, 0.0], [2.0, 0.0], [2.0, 2.0]],
            vec![[1.0, 1.0], [1.5, 1.0], [1.5, 1.5]],
        ];

        let mut buffer = FloatFlatContoursBuffer::<[f64; 2]>::default();
        buffer.set_with_resource(&shape);

        assert_eq!(buffer.ranges, vec![0..3, 3..6]);
        assert_eq!(buffer.to_contours(), shape);
    }

    #[test]
    fn test_set_with_iter() {
        let points = [
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 2.0],
            [10.0, 10.0],
            [11.0, 10.0],
            [11.0, 11.0],
        ];
        let ranges = [0..3, 3..6];

        let mut buffer = FloatFlatContoursBuffer::<[f64; 2]>::default();
        buffer.set_with_iter(points, &ranges);

        assert_eq!(buffer.points, points);
        assert_eq!(buffer.ranges, ranges);
    }

    #[test]
    fn test_shapes_add_contour() {
        let mut buffer = FloatFlatShapesBuffer::<[f64; 2]>::default();
        let c0 = [[0.0, 0.0], [2.0, 0.0], [2.0, 2.0]];
        let c1 = [[10.0, 10.0], [11.0, 10.0], [11.0, 11.0]];

        buffer.add_contour(&c0);
        buffer.add_contour(&c1);

        assert_eq!(buffer.contour_ranges.len(), 2);
        assert_eq!(buffer.contour_ranges[0], 0..3);
        assert_eq!(buffer.contour_ranges[1], 3..6);
        assert_eq!(buffer.shape_ranges, vec![0..1, 1..2]);
        assert_eq!(buffer.points.len(), 6);
        assert_eq!(buffer.to_shapes(), vec![vec![c0.to_vec()], vec![c1.to_vec()]]);
    }

    #[test]
    fn test_shapes_set_with_resource() {
        let shape: Vec<Vec<[f64; 2]>> = vec![
            vec![[0.0, 0.0], [2.0, 0.0], [2.0, 2.0]],
            vec![[1.0, 1.0], [1.5, 1.0], [1.5, 1.5]],
        ];

        let mut buffer = FloatFlatShapesBuffer::<[f64; 2]>::default();
        buffer.set_with_resource(&shape);

        assert_eq!(buffer.shape_ranges, vec![0..2]);
        assert_eq!(buffer.contour_ranges, vec![0..3, 3..6]);
        assert_eq!(buffer.to_shapes(), vec![shape]);
    }

    #[test]
    fn test_shapes_set_with_iter() {
        let points = [
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 2.0],
            [10.0, 10.0],
            [11.0, 10.0],
            [11.0, 11.0],
        ];
        let contour_ranges = [0..3, 3..6];
        let shape_ranges = [0..1, 1..2];

        let mut buffer = FloatFlatShapesBuffer::<[f64; 2]>::default();
        buffer.set_with_iter(points, &contour_ranges, &shape_ranges);

        assert_eq!(buffer.points, points);
        assert_eq!(buffer.contour_ranges, contour_ranges);
        assert_eq!(buffer.shape_ranges, shape_ranges);
    }
}
