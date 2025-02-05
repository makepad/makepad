use {
    super::{
        geometry::{Point, Rect, Size},
        numeric::Zero,
    },
    std::ops::{Index, IndexMut},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Image<T> {
    size: Size<usize>,
    pixels: Vec<T>,
}

impl<T> Image<T> {
    pub fn new(size: Size<usize>) -> Self
    where
        T: Clone + Default,
    {
        Self {
            size,
            pixels: vec![Default::default(); size.width * size.height],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.size() == Size::ZERO
    }

    pub fn size(&self) -> Size<usize> {
        self.size
    }

    pub fn pixels(&self) -> &[T] {
        &self.pixels
    }

    pub fn subimage(&self, rect: Rect<usize>) -> Subimage<'_, T> {
        Subimage {
            image: self,
            bounds: rect,
        }
    }

    pub fn subimage_mut(&mut self, rect: Rect<usize>) -> SubimageMut<'_, T> {
        assert!(
            Rect::from(self.size).contains_rect(rect),
            "rect is out of bounds"
        );
        SubimageMut {
            image: self,
            bounds: rect,
        }
    }
}

impl<T> Index<Point<usize>> for Image<T> {
    type Output = T;

    fn index(&self, point: Point<usize>) -> &Self::Output {
        assert!(
            Rect::from(self.size()).contains_point(point),
            "point is out of bounds"
        );
        &self.pixels[point.y * self.size.width + point.x]
    }
}

impl<T> IndexMut<Point<usize>> for Image<T> {
    fn index_mut(&mut self, point: Point<usize>) -> &mut Self::Output {
        assert!(
            Rect::from(self.size()).contains_point(point),
            "point is out of bounds"
        );
        &mut self.pixels[point.y * self.size.width + point.x]
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Subimage<'a, T> {
    image: &'a Image<T>,
    bounds: Rect<usize>,
}

impl<'a, T> Subimage<'a, T> {
    pub fn is_empty(&self) -> bool {
        self.bounds().is_empty()
    }

    pub fn bounds(&self) -> Rect<usize> {
        self.bounds
    }
}

impl<'a, T> Index<Point<usize>> for Subimage<'a, T> {
    type Output = T;

    fn index(&self, point: Point<usize>) -> &Self::Output {
        assert!(
            Rect::from(self.bounds().size).contains_point(point),
            "point is out of bounds"
        );
        &self.image[self.bounds.origin + Size::from(point)]
    }
}

#[derive(Debug, Eq, Hash, PartialEq)]
pub struct SubimageMut<'a, T> {
    image: &'a mut Image<T>,
    bounds: Rect<usize>,
}

impl<'a, T> SubimageMut<'a, T> {
    pub fn is_empty(&self) -> bool {
        self.bounds().is_empty()
    }

    pub fn bounds(&self) -> Rect<usize> {
        self.bounds
    }
}

impl<'a, T> Index<Point<usize>> for SubimageMut<'a, T> {
    type Output = T;

    fn index(&self, point: Point<usize>) -> &Self::Output {
        assert!(
            Rect::from(self.bounds().size).contains_point(point),
            "point is out of bounds"
        );
        &self.image[self.bounds.origin + Size::from(point)]
    }
}

impl<'a, T> IndexMut<Point<usize>> for SubimageMut<'a, T> {
    fn index_mut(&mut self, point: Point<usize>) -> &mut Self::Output {
        assert!(
            Rect::from(self.bounds().size).contains_point(point),
            "point is out of bounds"
        );
        &mut self.image[self.bounds.origin + Size::from(point)]
    }
}
