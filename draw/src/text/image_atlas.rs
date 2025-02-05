use super::{
    geometry::{Point, Rect, Size},
    image::{Image, Subimage, SubimageMut},
    numeric::Zero,
    pixels::{Bgra, R},
};

#[derive(Clone, Debug)]
pub struct ImageAtlas<T> {
    image: Image<T>,
    dirty_rect: Rect<usize>,
    current_point: Point<usize>,
    current_row_height: usize,
}

impl<T> ImageAtlas<T> {
    pub fn new(size: Size<usize>) -> Self
    where
        T: Clone + Default,
    {
        Self {
            image: Image::new(size),
            dirty_rect: Rect::ZERO,
            current_point: Point::ZERO,
            current_row_height: 0,
        }
    }

    pub fn size(&self) -> Size<usize> {
        self.image.size()
    }

    pub fn image(&self) -> &Image<T> {
        &self.image
    }

    pub fn take_dirty_image(&mut self) -> Subimage<'_, T> {
        let dirty_rect = self.dirty_rect;
        self.dirty_rect = Rect::ZERO;
        self.image.subimage(dirty_rect)
    }

    pub fn allocate_image(&mut self, size: Size<usize>) -> Option<SubimageMut<'_, T>> {
        const PADDING: Size<usize> = Size::new(2, 2);

        let padded_size = size + PADDING;
        if self.current_point.x + padded_size.width > self.size().width {
            self.current_point.x = 0;
            self.current_point.y += self.current_row_height;
            self.current_row_height = 0;
        }
        if self.current_point.y + padded_size.height > self.size().height {
            return None;
        }
        let origin = self.current_point;
        self.current_point.x += padded_size.width;
        self.current_row_height = self.current_row_height.max(padded_size.height);
        let rect = Rect::new(origin, size);
        self.dirty_rect = self.dirty_rect.union(rect);
        Some(self.image.subimage_mut(rect))
    }
}

pub type GrayscaleAtlas = ImageAtlas<R<u8>>;
pub type ColorAtlas = ImageAtlas<Bgra<u8>>;
