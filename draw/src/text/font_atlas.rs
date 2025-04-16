use {
    super::{
        font::{FontId, GlyphId},
        geom::{Point, Rect, Size},
        image::{Image, Rgba, Subimage, SubimageMut, R},
        num::Zero,
    },
    std::{collections::HashMap, fs::File, io::BufWriter, path::Path, slice},
};

#[derive(Clone, Debug)]
pub struct FontAtlas<T> {
    needs_reset: bool,
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
            needs_reset: false,
            image: Image::new(size),
            dirty_rect: Rect::ZERO,
            current_point: Point::ZERO,
            current_row_height: 0,
            cached_glyph_image_rects: HashMap::new(),
        }
    }

    pub fn needs_reset(&self) -> bool {
        self.needs_reset
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

    pub fn get_or_allocate_glyph_image(&mut self, key: GlyphImageKey) -> Option<GlyphImage<'_, T>> {
        if let Some(rect) = self.cached_glyph_image_rects.get(&key) {
            return Some(GlyphImage::Cached(*rect));
        }
        let rect = self.allocate_glyph_image(key.size)?;
        self.cached_glyph_image_rects.insert(key.clone(), rect);
        Some(GlyphImage::Allocated(self.image.subimage_mut(rect)))
    }

    fn allocate_glyph_image(&mut self, size: Size<usize>) -> Option<Rect<usize>> {
        const PADDING: Size<usize> = Size::new(2, 2);

        let padded_size = size + PADDING;
        if self.current_point.x + padded_size.width > self.size().width {
            self.current_point.x = 0;
            self.current_point.y += self.current_row_height;
            self.current_row_height = 0;
        }
        if self.current_point.y + padded_size.height > self.size().height {
            self.needs_reset = true;
            crate::log!("Font atlas too small, resetting");
            return None;
        }
        let origin = self.current_point;
        self.current_point.x += padded_size.width;
        self.current_row_height = self.current_row_height.max(padded_size.height);
        let rect = Rect::new(origin, size);
        self.dirty_rect = self.dirty_rect.union(rect);
        Some(rect)
    }

    pub fn reset(&mut self) {
        self.needs_reset = false;
        self.dirty_rect = Rect::ZERO;
        self.current_point = Point::ZERO;
        self.current_row_height = 0;
        self.cached_glyph_image_rects.clear();
    }
}

pub type GrayscaleAtlas = FontAtlas<R>;

impl GrayscaleAtlas {
    pub fn save_to_png(&self, path: impl AsRef<Path>) {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let size = self.size();
        let mut encoder = png::Encoder::new(writer, size.width as u32, size.height as u32);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let pixels = self.image.as_pixels();
        let data =
            unsafe { slice::from_raw_parts(pixels.as_ptr() as *const u8, pixels.len()) };
        writer.write_image_data(&data).unwrap();
    }
}

pub type ColorAtlas = FontAtlas<Rgba>;

impl ColorAtlas {
    pub fn save_to_png(&self, path: impl AsRef<Path>) {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let size = self.size();
        let mut encoder = png::Encoder::new(writer, size.width as u32, size.height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let pixels = self.image.as_pixels();
        let data =
            unsafe { slice::from_raw_parts(pixels.as_ptr() as *const u8, pixels.len() * 4) };
        writer.write_image_data(&data).unwrap();
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct GlyphImageKey {
    pub font_id: FontId,
    pub glyph_id: GlyphId,
    pub size: Size<usize>,
}

#[derive(Debug)]
pub enum GlyphImage<'a, T> {
    Cached(Rect<usize>),
    Allocated(SubimageMut<'a, T>),
}
