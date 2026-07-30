use crate::int::number::int::IntNumber;
use crate::int::point::IntPoint;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub struct IntRect<T: IntNumber = i32> {
    pub min_x: T,
    pub max_x: T,
    pub min_y: T,
    pub max_y: T,
}

impl<T: IntNumber> IntRect<T> {
    #[inline(always)]
    pub fn width(&self) -> T {
        self.max_x - self.min_x
    }

    #[inline(always)]
    pub fn height(&self) -> T {
        self.max_y - self.min_y
    }

    #[inline(always)]
    pub fn new(min_x: T, max_x: T, min_y: T, max_y: T) -> Self {
        Self {
            min_x,
            max_x,
            min_y,
            max_y,
        }
    }

    #[inline(always)]
    pub fn with_min_max(min: IntPoint<T>, max: IntPoint<T>) -> Self {
        Self {
            min_x: min.x,
            max_x: max.x,
            min_y: min.y,
            max_y: max.y,
        }
    }

    #[inline]
    pub fn with_point(point: IntPoint<T>) -> Self {
        Self {
            min_x: point.x,
            max_x: point.x,
            min_y: point.y,
            max_y: point.y,
        }
    }

    #[inline]
    pub fn with_ab(a: IntPoint<T>, b: IntPoint<T>) -> Self {
        let (min_x, max_x) = if a.x < b.x { (a.x, b.x) } else { (b.x, a.x) };
        let (min_y, max_y) = if a.y < b.y { (a.y, b.y) } else { (b.y, a.y) };

        Self {
            min_x,
            max_x,
            min_y,
            max_y,
        }
    }

    #[inline]
    pub fn with_points(points: &[IntPoint<T>]) -> Option<Self> {
        Self::with_iter(points.iter())
    }

    pub fn with_iter<'a, I: Iterator<Item = &'a IntPoint<T>>>(iter: I) -> Option<Self>
    where
        T: 'a,
    {
        let mut iter = iter;
        let first_point = iter.next()?;

        let mut rect = Self {
            min_x: first_point.x,
            max_x: first_point.x,
            min_y: first_point.y,
            max_y: first_point.y,
        };

        for p in iter {
            rect.unsafe_add_point(p);
        }

        Some(rect)
    }

    #[inline]
    pub fn with_rects(rect0: &Self, rect1: &Self) -> Self {
        let min_x = rect0.min_x.min(rect1.min_x);
        let max_x = rect0.max_x.max(rect1.max_x);
        let min_y = rect0.min_y.min(rect1.min_y);
        let max_y = rect0.max_y.max(rect1.max_y);

        Self::new(min_x, max_x, min_y, max_y)
    }

    #[inline]
    pub fn with_optional_rects(rect0: Option<Self>, rect1: Option<Self>) -> Option<Self> {
        match (rect0, rect1) {
            (Some(r0), Some(r1)) => Some(Self::with_rects(&r0, &r1)),
            (Some(r0), None) => Some(r0),
            (None, Some(r1)) => Some(r1),
            (None, None) => None,
        }
    }

    #[inline]
    pub fn add_point(&mut self, point: &IntPoint<T>) {
        self.max_x = self.max_x.max(point.x);
        self.min_x = self.min_x.min(point.x);
        self.max_y = self.max_y.max(point.y);
        self.min_y = self.min_y.min(point.y);
    }

    #[inline]
    pub fn unsafe_add_point(&mut self, point: &IntPoint<T>) {
        if self.min_x > point.x {
            self.min_x = point.x
        } else if self.max_x < point.x {
            self.max_x = point.x
        }

        if self.min_y > point.y {
            self.min_y = point.y
        } else if self.max_y < point.y {
            self.max_y = point.y
        }
    }

    #[inline(always)]
    pub fn contains(&self, point: IntPoint<T>) -> bool {
        self.min_x <= point.x && point.x <= self.max_x && self.min_y <= point.y && point.y <= self.max_y
    }

    #[inline(always)]
    pub fn contains_exclude_borders(&self, point: IntPoint<T>) -> bool {
        self.min_x < point.x && point.x < self.max_x && self.min_y < point.y && point.y < self.max_y
    }

    #[inline(always)]
    pub fn contains_with_radius(&self, point: IntPoint<T>, radius: T) -> bool {
        let min_x = self.min_x - radius;
        let max_x = self.max_x + radius;
        let min_y = self.min_y - radius;
        let max_y = self.max_y + radius;
        min_x <= point.x && point.x <= max_x && min_y <= point.y && point.y <= max_y
    }

    #[inline]
    pub fn is_intersect_border_include(&self, other: &Self) -> bool {
        let x = self.min_x <= other.max_x && self.max_x >= other.min_x;
        let y = self.min_y <= other.max_y && self.max_y >= other.min_y;
        x && y
    }

    #[inline]
    pub fn is_intersect_border_exclude(&self, other: &Self) -> bool {
        let x = self.min_x < other.max_x && self.max_x > other.min_x;
        let y = self.min_y < other.max_y && self.max_y > other.min_y;
        x && y
    }

    #[inline]
    pub fn contains_rect(&self, other: &Self) -> bool {
        let x = self.min_x <= other.min_x && other.max_x <= self.max_x;
        let y = self.min_y <= other.min_y && other.max_y <= self.max_y;
        x && y
    }
}

#[cfg(test)]
mod tests {
    use crate::int::point::IntPoint;
    use crate::int::rect::IntRect;

    #[test]
    fn test_0() {
        let rect = if let Some(rect) =
            IntRect::with_points(&[IntPoint::new(0, 0), IntPoint::new(-7, 10), IntPoint::new(20, -5)])
        {
            rect
        } else {
            return;
        };

        assert_eq!(rect.min_x, -7);
        assert_eq!(rect.max_x, 20);
        assert_eq!(rect.min_y, -5);
        assert_eq!(rect.max_y, 10);
    }

    #[test]
    fn test_1() {
        let rect = IntRect::new(-10, 10, -10, 10);

        assert_eq!(rect.contains(IntPoint::new(-20, -20)), false);
        assert_eq!(rect.contains(IntPoint::new(-20, -10)), false);
        assert_eq!(rect.contains(IntPoint::new(-20, 0)), false);
        assert_eq!(rect.contains(IntPoint::new(-20, 10)), false);
        assert_eq!(rect.contains(IntPoint::new(-20, 20)), false);

        assert_eq!(rect.contains(IntPoint::new(-10, -20)), false);
        assert_eq!(rect.contains(IntPoint::new(-10, -10)), true);
        assert_eq!(rect.contains(IntPoint::new(-10, 0)), true);
        assert_eq!(rect.contains(IntPoint::new(-10, 10)), true);
        assert_eq!(rect.contains(IntPoint::new(-10, 20)), false);

        assert_eq!(rect.contains(IntPoint::new(0, -20)), false);
        assert_eq!(rect.contains(IntPoint::new(0, -10)), true);
        assert_eq!(rect.contains(IntPoint::new(0, 0)), true);
        assert_eq!(rect.contains(IntPoint::new(0, 10)), true);
        assert_eq!(rect.contains(IntPoint::new(0, 20)), false);

        assert_eq!(rect.contains(IntPoint::new(10, -20)), false);
        assert_eq!(rect.contains(IntPoint::new(10, -10)), true);
        assert_eq!(rect.contains(IntPoint::new(10, 0)), true);
        assert_eq!(rect.contains(IntPoint::new(10, 10)), true);
        assert_eq!(rect.contains(IntPoint::new(10, 20)), false);

        assert_eq!(rect.contains(IntPoint::new(20, -20)), false);
        assert_eq!(rect.contains(IntPoint::new(20, -10)), false);
        assert_eq!(rect.contains(IntPoint::new(20, 0)), false);
        assert_eq!(rect.contains(IntPoint::new(20, 10)), false);
        assert_eq!(rect.contains(IntPoint::new(20, 20)), false);
    }

    #[test]
    fn test_generic_i64() {
        let rect = IntRect::with_points(&[IntPoint::<i64>::new(-1, 4), IntPoint::<i64>::new(8, -2)]).unwrap();

        assert_eq!(rect.width(), 9);
        assert_eq!(rect.height(), 6);
        assert!(rect.contains(IntPoint::new(8, -2)));
        assert!(rect.contains_with_radius(IntPoint::new(10, -2), 2));
        assert!(!rect.contains_with_radius(IntPoint::new(11, -2), 2));
    }
}
