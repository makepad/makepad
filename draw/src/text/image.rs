use {
    super::{
        geom::{Point, Rect, Size},
        num::Zero,
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

    pub fn from_size_and_pixels(size: Size<usize>, pixels: Vec<T>) -> Self {
        assert_eq!(size.width * size.height, pixels.len());
        Self { size, pixels }
    }

    pub fn into_pixels(self) -> Vec<T> {
        self.pixels
    }

    pub fn is_empty(&self) -> bool {
        self.size() == Size::ZERO
    }

    pub fn size(&self) -> Size<usize> {
        self.size
    }

    pub fn as_pixels(&self) -> &[T] {
        &self.pixels
    }

    pub fn as_mut_pixels(&mut self) -> &mut [T] {
        &mut self.pixels
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

    pub fn size(&self) -> Size<usize> {
        self.bounds().size
    }

    pub fn bounds(&self) -> Rect<usize> {
        self.bounds
    }

    pub fn to_image(&self) -> Image<T>
    where
        T: Copy,
    {
        let mut pixels = Vec::with_capacity(self.size().width * self.size().height);
        for y in 0..self.size().height {
            for x in 0..self.size().width {
                pixels.push(self[Point::new(x, y)]);
            }
        }
        Image::from_size_and_pixels(self.size(), pixels)
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

    pub fn size(&self) -> Size<usize> {
        self.bounds().size
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

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(C)]
pub struct R {
    pub r: u8,
}

impl R {
    pub const fn new(r: u8) -> Self {
        Self { r }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(C)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}
