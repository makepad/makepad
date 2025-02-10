use {
    super::{
        font::{FontId, GlyphId},
        geom::{Point, Rect, Size},
        image::{Image, Subimage, SubimageMut},
        num::Zero,
        pixels::{Rgba, R},
    },
    std::collections::HashMap,
};

#[derive(Clone, Debug)]
pub struct FontAtlas<T> {
    image: Image<T>,
    dirty_rect: Rect<usize>,
    current_point: Point<usize>,
    current_row_height: usize,
    cached_glyph_image_rects: HashMap<GlyphImageKey, Rect<usize>>,
}

impl<T> FontAtlas<T> {
    pub fn new(size: Size<usize>) -> Self
    where
        T: Clone + Default,
    {
        Self {
            image: Image::new(size),
            dirty_rect: Rect::ZERO,
            current_point: Point::ZERO,
            current_row_height: 0,
            cached_glyph_image_rects: HashMap::new(),
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

    pub fn get_or_allocate_glyph_image(
        &mut self,
        key: GlyphImageKey,
    ) -> Option<SubimageMut<'_, T>> {
        if !self.cached_glyph_image_rects.contains_key(&key) {
            let rect = self.allocate_glyph_image(key.size)?;
            self.cached_glyph_image_rects.insert(key.clone(), rect);
        }
        self.cached_glyph_image_rects
            .get(&key)
            .copied()
            .map(|rect| self.image.subimage_mut(rect))
    }

    pub fn allocate_glyph_image(&mut self, size: Size<usize>) -> Option<Rect<usize>> {
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
        Some(rect)
    }
}

pub type GrayscaleAtlas = FontAtlas<R<u8>>;
pub type ColorAtlas = FontAtlas<Rgba<u8>>;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct GlyphImageKey {
    pub font_id: FontId,
    pub glyph_id: GlyphId,
    pub size: Size<usize>,
}
