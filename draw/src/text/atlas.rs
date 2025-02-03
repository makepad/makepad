use super::{
    geom::{Point, Rect, Size},
    image::{Image, Subimage, SubimageMut},
    num::Zero,
};

#[derive(Clone, Debug)]
pub struct Atlas<T> {
    image: Image<T>,
    dirty_rect: Rect<usize>,
    next_origin: Point<usize>,
    current_row_height: usize,
}

impl<T> Atlas<T> {
    pub fn new(size: Size<usize>) -> Self
    where
        T: Clone + Default,
    {
        Self {
            image: Image::new(size),
            dirty_rect: Rect::ZERO,
            next_origin: Point::ZERO,
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
        if self.next_origin.x + padded_size.width > self.size().width {
            self.next_origin.x = 0;
            self.next_origin.y += self.current_row_height;
            self.current_row_height = 0;
        }
        if self.next_origin.y + padded_size.height > self.size().height {
            return None;
        }
        let origin = self.next_origin;
        self.next_origin.x += padded_size.width;
        self.current_row_height = self.current_row_height.max(padded_size.height);
        let rect = Rect::new(origin, size);
        self.dirty_rect = self.dirty_rect.union(rect);
        Some(self.image.subimage_mut(rect))
    }
}
