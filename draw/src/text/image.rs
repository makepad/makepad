use {
    super::{
        geom::{Point, Rect, Size},
        num::Zero,
    },
    std::{mem, ops::{Index, IndexMut}},
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

    pub unsafe fn replace_pixels(&mut self, pixels: Vec<T>) -> Vec<T> {
        mem::replace(&mut self.pixels, pixels)
    }

    pub fn subimage(&self, rect: Rect<usize>) -> Subimage<'_, T> {
        assert!(
            Rect::from(self.size).contains_rect(rect),
            "rect is out of bounds"
        );
        Subimage {
            image: self,
            bounds: rect,
        }
    }

    pub fn subimage_mut(&mut self, rect: Rect<usize>) -> SubimageMut<'_, T> {
        assert!(
            Rect::from(self.size()).contains_rect(rect),
            "rect {:?} is out of bounds (should fit in rect {:?})",
            rect,
            Rect::from(self.size())
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

    pub fn subimage_mut(self, rect: Rect<usize>) -> SubimageMut<'a, T> {
        assert!(
            Rect::from(self.size()).contains_rect(rect),
            "rect {:?} is out of bounds (should fit in rect {:?})",
            rect,
            Rect::from(self.size())
        );
        SubimageMut {
            image: self.image,
            bounds: Rect::new(self.bounds.origin + Size::from(rect.origin), rect.size),
        }
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
#[repr(transparent)]
pub struct R {
    pub bits: u8,
}

impl R {
    pub const fn new(r: u8) -> Self {
        Self { bits: r }
    }

    pub fn r(self) -> u8 {
        self.bits
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct Bgra {
    pub bits: u32,
}

impl Bgra {
    pub fn new(b: u8, g: u8, r: u8, a: u8) -> Self {
        let b = u32::from(b);
        let g = u32::from(g);
        let r = u32::from(r);
        let a = u32::from(a);
        Self { bits: (a << 24) | (r << 16) | (g << 8) | b }
    }

    pub fn b(self) -> u8 {
        (self.bits & 0xFF) as u8
    }

    pub fn g(self) -> u8 {
        ((self.bits >> 8) & 0xFF) as u8
    }

    pub fn r(self) -> u8 {
        ((self.bits >> 16) & 0xFF) as u8
    }

    pub fn a(self) -> u8 {
        ((self.bits >> 24) & 0xFF) as u8
    }
}
