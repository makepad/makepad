use crate::float::compatible::FloatPointCompatible;
use crate::float::number::FloatNumber;
use core::fmt;

#[derive(Debug, Copy, Clone)]
pub struct FloatRect<T: FloatNumber> {
    pub min_x: T,
    pub max_x: T,
    pub min_y: T,
    pub max_y: T,
}

impl<T: FloatNumber> FloatRect<T> {
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
    pub fn zero() -> Self {
        let zero = FloatNumber::from_float(0.0);
        Self {
            min_x: zero,
            max_x: zero,
            min_y: zero,
            max_y: zero,
        }
    }

    #[inline]
    pub fn with_point<P: FloatPointCompatible<Scalar = T>>(point: P) -> Self {
        Self {
            min_x: point.x(),
            max_x: point.x(),
            min_y: point.y(),
            max_y: point.y(),
        }
    }

    #[inline]
    pub fn with_points<P>(points: &[P]) -> Option<Self>
    where
        P: FloatPointCompatible<Scalar = T>,
    {
        Self::with_iter(points.iter())
    }

    #[inline]
    pub fn with_iter<'a, I, P>(iter: I) -> Option<Self>
    where
        I: Iterator<Item = &'a P>,
        P: FloatPointCompatible<Scalar = T> + 'a,
        T: FloatNumber,
    {
        let mut iter = iter;
        let first_point = iter.next()?;
        let mut rect = Self {
            min_x: first_point.x(),
            max_x: first_point.x(),
            min_y: first_point.y(),
            max_y: first_point.y(),
        };

        for p in iter {
            rect.unsafe_add_point(p);
        }

        Some(rect)
    }

    #[inline]
    pub fn with_rects(rect_0: Self, rect_1: Self) -> Self {
        let min_x = rect_0.min_x.min(rect_1.min_x);
        let max_x = rect_0.max_x.max(rect_1.max_x);
        let min_y = rect_0.min_y.min(rect_1.min_y);
        let max_y = rect_0.max_y.max(rect_1.max_y);
        FloatRect::new(min_x, max_x, min_y, max_y)
    }

    #[inline]
    pub fn with_optional_rects(rect_0: Option<Self>, rect_1: Option<Self>) -> Option<Self> {
        match (rect_0, rect_1) {
            (Some(r0), Some(r1)) => Some(Self::with_rects(r0, r1)),
            (Some(r0), None) => Some(r0),
            (None, Some(r1)) => Some(r1),
            (None, None) => None,
        }
    }

    #[inline]
    pub fn add_point<P: FloatPointCompatible<Scalar = T>>(&mut self, point: &P) {
        if self.min_x > point.x() {
            self.min_x = point.x()
        }
        if self.max_x < point.x() {
            self.max_x = point.x()
        }

        if self.min_y > point.y() {
            self.min_y = point.y()
        }
        if self.max_y < point.y() {
            self.max_y = point.y()
        }
    }

    #[inline]
    pub fn add_offset(&mut self, offset: T) {
        self.max_x = self.max_x + offset;
        self.max_y = self.max_y + offset;
        self.min_x = self.min_x - offset;
        self.min_y = self.min_y - offset;
    }

    #[inline]
    pub fn unsafe_add_point<P: FloatPointCompatible<Scalar = T>>(&mut self, point: &P) {
        if self.min_x > point.x() {
            self.min_x = point.x()
        } else if self.max_x < point.x() {
            self.max_x = point.x()
        }

        if self.min_y > point.y() {
            self.min_y = point.y()
        } else if self.max_y < point.y() {
            self.max_y = point.y()
        }
    }

    #[inline]
    pub fn optional_add_point<P: FloatPointCompatible<Scalar = T>>(rect: &mut Option<Self>, point: &P) {
        match rect {
            Some(rect) => rect.unsafe_add_point(point),
            None => *rect = Some(FloatRect::with_point(*point)),
        }
    }

    #[inline(always)]
    pub fn contains<P: FloatPointCompatible<Scalar = T>>(&self, point: &P) -> bool {
        self.min_x <= point.x()
            && point.x() <= self.max_x
            && self.min_y <= point.y()
            && point.y() <= self.max_y
    }

    #[inline(always)]
    pub fn contains_with_radius<P: FloatPointCompatible<Scalar = T>>(&self, point: &P, radius: T) -> bool {
        let min_x = self.min_x - radius;
        let max_x = self.max_x + radius;
        let min_y = self.min_y - radius;
        let max_y = self.max_y + radius;
        min_x <= point.x() && point.x() <= max_x && min_y <= point.y() && point.y() <= max_y
    }
}

impl<T: FloatNumber> fmt::Display for FloatRect<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "({}, {})-({}, {})",
            self.min_x, self.min_y, self.max_x, self.max_y
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::float::rect::FloatRect;

    #[test]
    fn test_0() {
        let points = [[-2.0, -4.0], [-2.0, 3.0], [5.0, 3.0], [5.0, -4.0]];

        let rect: FloatRect<f64> = FloatRect::with_iter(points.iter()).unwrap();

        assert_eq!((rect.max_x - 5.0).abs() < 0.000_0001, true);
        assert_eq!((rect.min_x + 2.0).abs() < 0.000_0001, true);
        assert_eq!((rect.max_y - 3.0).abs() < 0.000_0001, true);
        assert_eq!((rect.min_y + 4.0).abs() < 0.000_0001, true);
    }

    #[test]
    fn test_1() {
        let r0 = Some(FloatRect::new(-2.0, 2.0, -2.0, 2.0));
        let r1 = Some(FloatRect::new(-4.0, 4.0, -4.0, 4.0));
        let rr = FloatRect::with_optional_rects(r0, r1).unwrap();

        assert_eq!(-4.0, rr.min_x);
        assert_eq!(-4.0, rr.min_y);
        assert_eq!(4.0, rr.max_x);
        assert_eq!(4.0, rr.max_y);
        assert!(FloatRect::with_optional_rects(r0, None).is_some());
        assert!(FloatRect::with_optional_rects(None, r1).is_some());
        assert!(FloatRect::<f32>::with_optional_rects(None, None).is_none());
    }
}
