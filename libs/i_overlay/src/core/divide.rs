use crate::geom::id_point::IdPoint;
use alloc::vec;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_float::int::point::IntPoint;
use i_shape::int::path::IntPath;
use i_shape::int::shape::IntContour;

struct SubPath<I: IntNumber> {
    last: usize,
    node: IntPoint<I>,
    path: IntPath<I>,
}

impl<I: IntNumber> SubPath<I> {
    fn start(point: IdPoint<I>) -> Self {
        Self {
            last: point.id + 1,
            node: point.point,
            path: vec![point.point],
        }
    }

    fn join(&mut self, point: IdPoint<I>, source: &IntContour<I>) {
        self.path.extend_from_slice(&source[self.last..point.id]);
        self.last = point.id;
    }

    fn shift(&mut self, point: IdPoint<I>) {
        self.last = point.id;
    }
}

pub trait ContourDecomposition<I: IntNumber> {
    fn decompose_contours(&self) -> Option<Vec<IntContour<I>>>;
}

impl<I: IntNumber> ContourDecomposition<I> for IntContour<I> {
    fn decompose_contours(&self) -> Option<Vec<IntContour<I>>> {
        if self.len() < 3 {
            return None;
        }
        let mut id_points: Vec<_> = self
            .iter()
            .enumerate()
            .map(|(i, &p)| IdPoint::new(i, p))
            .collect();

        id_points.sort_unstable_by(|p0, p1| p0.point.cmp(&p1.point).then_with(|| p0.id.cmp(&p1.id)));

        let mut p0 = id_points.first().unwrap().point;
        let mut anchors = Vec::new();
        let mut n = 0;
        for (i, idp) in id_points.iter().enumerate().skip(1) {
            if p0 == idp.point {
                n += 1;
                continue;
            }

            if n > 0 {
                anchors.extend_from_slice(&id_points[i - n - 1..i]);
                n = 0;
            }

            p0 = idp.point;
        }

        if anchors.is_empty() {
            return None;
        }

        anchors.sort_by_key(|p0| p0.id);

        let mut contours = Vec::with_capacity((anchors.len() >> 1) + 1);

        let mut queue = vec![];

        let mut i = 0;
        while i < anchors.len() {
            let a = anchors[i];
            let mut sub_path: SubPath<I> = if let Some(sub_path) = queue.pop() {
                sub_path
            } else {
                queue.push(SubPath::<I>::start(a));
                i += 1;
                continue;
            };

            if sub_path.node == a.point {
                sub_path.join(a, self);
                contours.push(sub_path.path);
                if let Some(prev) = queue.last_mut() {
                    prev.shift(a);
                } else {
                    queue.push(SubPath::<I>::start(a));
                }
            } else {
                sub_path.join(a, self);
                queue.push(sub_path);
                queue.push(SubPath::<I>::start(a));
            }
            i += 1;
        }

        let mut sub_path: SubPath<I> = queue.pop().unwrap();

        if sub_path.last < self.len() {
            sub_path.path.extend_from_slice(&self[sub_path.last..]);
        }

        let i0 = anchors.first().unwrap().id;
        if i0 > 0 {
            sub_path.path.extend_from_slice(&self[..i0]);
        }

        contours.push(sub_path.path);

        Some(contours)
    }
}

#[cfg(test)]
mod tests {
    use crate::core::divide::ContourDecomposition;
    use alloc::vec;
    use i_float::int::point::IntPoint;
    use i_shape::int::shape::IntContour;

    #[test]
    fn test_0() {
        let origin = vec![
            IntPoint::new(0, 0),
            IntPoint::new(0, 2),
            IntPoint::new(2, 0),
            IntPoint::new(4, 2),
            IntPoint::new(4, 0),
            IntPoint::new(2, 0),
        ];

        let contours = origin.decompose_contours().unwrap();

        assert_eq!(contours.len(), 2);
    }

    #[test]
    fn test_0_rotate() {
        let origin = vec![
            IntPoint::new(0, 0),
            IntPoint::new(0, 2),
            IntPoint::new(2, 0),
            IntPoint::new(4, 2),
            IntPoint::new(4, 0),
            IntPoint::new(2, 0),
        ];

        for i in 0..origin.len() {
            let contour = rotate(&origin, i);
            let contours = contour.decompose_contours().unwrap();
            assert_eq!(contours.len(), 2);
            assert_eq!(contours.iter().fold(0, |s, c| s + c.len()), origin.len());
        }
    }

    #[test]
    fn test_1_0() {
        let origin = vec![
            IntPoint::new(0, 0),
            IntPoint::new(-2, 2),
            IntPoint::new(0, 2),
            IntPoint::new(-2, 4),
            IntPoint::new(0, 4),
            IntPoint::new(-2, 6),
            IntPoint::new(2, 6),
            IntPoint::new(0, 4),
            IntPoint::new(2, 4),
            IntPoint::new(0, 2),
            IntPoint::new(2, 2),
        ];

        let contours = origin.decompose_contours().unwrap();

        assert_eq!(contours.len(), 3);
    }

    #[test]
    fn test_1_1() {
        let origin = vec![
            IntPoint::new(-2, 4),
            IntPoint::new(0, 4),
            IntPoint::new(-2, 6),
            IntPoint::new(2, 6),
            IntPoint::new(0, 4),
            IntPoint::new(2, 4),
            IntPoint::new(0, 2),
            IntPoint::new(2, 2),
            IntPoint::new(0, 0),
            IntPoint::new(-2, 2),
            IntPoint::new(0, 2),
        ];

        let contours = origin.decompose_contours().unwrap();

        assert_eq!(contours.len(), 3);
    }

    #[test]
    fn test_1_rotate() {
        let origin = vec![
            IntPoint::new(0, 0),
            IntPoint::new(-2, 2),
            IntPoint::new(0, 2),
            IntPoint::new(-2, 4),
            IntPoint::new(0, 4),
            IntPoint::new(-2, 6),
            IntPoint::new(2, 6),
            IntPoint::new(0, 4),
            IntPoint::new(2, 4),
            IntPoint::new(0, 2),
            IntPoint::new(2, 2),
        ];

        let n = origin.len();
        for i in 0..n {
            let contour = rotate(&origin, i);
            let contours = contour.decompose_contours().unwrap();
            assert_eq!(contours.len(), 3);
            let len = contours.iter().fold(0, |s, c| s + c.len());
            assert_eq!(len, n);
        }
    }

    #[test]
    fn test_2() {
        let origin = vec![
            IntPoint::new(0, 0),
            IntPoint::new(-2, -1),
            IntPoint::new(-2, 1),
            IntPoint::new(0, 0),
            IntPoint::new(-1, 2),
            IntPoint::new(1, 2),
            IntPoint::new(0, 0),
            IntPoint::new(2, 1),
            IntPoint::new(2, -1),
            IntPoint::new(0, 0),
            IntPoint::new(1, -2),
            IntPoint::new(-1, -2),
        ];

        let contours = origin.decompose_contours().unwrap();
        assert_eq!(contours.len(), 4);
    }

    #[test]
    fn test_2_rotate() {
        let origin = vec![
            IntPoint::new(0, 0),
            IntPoint::new(-2, -1),
            IntPoint::new(-2, 1),
            IntPoint::new(0, 0),
            IntPoint::new(-1, 2),
            IntPoint::new(1, 2),
            IntPoint::new(0, 0),
            IntPoint::new(2, 1),
            IntPoint::new(2, -1),
            IntPoint::new(0, 0),
            IntPoint::new(1, -2),
            IntPoint::new(-1, -2),
        ];

        let n = origin.len();
        for i in 0..n {
            let contour = rotate(&origin, i);
            let contours = contour.decompose_contours().unwrap();
            assert_eq!(contours.len(), 4);
            let len = contours.iter().fold(0, |s, c| s + c.len());
            assert_eq!(len, n);
        }
    }

    fn rotate(contour: &IntContour<i32>, s: usize) -> IntContour<i32> {
        contour
            .iter()
            .cycle()
            .skip(s)
            .take(contour.len())
            .cloned()
            .collect()
    }
}
