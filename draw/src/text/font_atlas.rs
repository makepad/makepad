use {
    super::{
        font::{FontId, GlyphId},
        geom::{Rect, Size},
        image::{Bgra, Image, Subimage, SubimageMut},
        num::Zero,
    },
};

#[derive(Clone, Debug)]
pub struct FontAtlas<T> {
    needs_reset: bool,
    image: Image<T>,
    dirty_rect: Rect<usize>,
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
        }
    }

    pub fn needs_reset(&self) -> bool {
        self.needs_reset
    }

    pub fn request_reset(&mut self) {
        self.needs_reset = true;
    }

    pub fn size(&self) -> Size<usize> {
        self.image.size()
    }

    pub fn dirty_rect(&self) -> Rect<usize> {
        self.dirty_rect
    }

    pub fn image(&self) -> &Image<T> {
        &self.image
    }

    pub fn take_pixels(&mut self) -> Vec<T> {
        self.image.take_pixels()
    }

    pub fn replace_pixels(&mut self, pixels: Vec<T>) -> Vec<T> {
        self.image.replace_pixels(pixels)
    }

    pub fn take_dirty_image(&mut self) -> Subimage<'_, T> {
        let dirty_rect = self.dirty_rect;
        self.dirty_rect = Rect::ZERO;
        self.image.subimage(dirty_rect)
    }

    pub fn get_cached_glyph_image_mut(&mut self, rect: Rect<usize>) -> SubimageMut<'_, T> {
        self.mark_dirty_rect(rect);
        self.image.subimage_mut(rect)
    }

    fn mark_dirty_rect(&mut self, rect: Rect<usize>) {
        if self.dirty_rect.is_empty() {
            self.dirty_rect = rect;
        } else {
            self.dirty_rect = self.dirty_rect.union(rect);
        }
    }

    pub fn reset_if_needed(&mut self) -> bool {
        if !self.needs_reset() {
            return false;
        }
        self.needs_reset = false;
        self.dirty_rect = Rect::ZERO;
        true
    }
}

pub type GrayscaleAtlas = FontAtlas<Bgra>;

// Debug save_to_png methods commented out - would require png encoder crate
// impl GrayscaleAtlas {
//     pub fn save_to_png(&self, path: impl AsRef<Path>) {
//         use std::{fs::File, io::BufWriter, slice};
//         let file = File::create(path).unwrap();
//         let writer = BufWriter::new(file);
//         let size = self.size();
//         let mut encoder = png::Encoder::new(writer, size.width as u32, size.height as u32);
//         encoder.set_color(png::ColorType::Grayscale);
//         encoder.set_depth(png::BitDepth::Eight);
//         let mut writer = encoder.write_header().unwrap();
//         let pixels = self.image.as_pixels();
//         let data = unsafe { slice::from_raw_parts(pixels.as_ptr() as *const u8, pixels.len()) };
//         writer.write_image_data(&data).unwrap();
//     }
// }

pub type ColorAtlas = FontAtlas<Bgra>;

pub type MsdfAtlas = FontAtlas<Bgra>;

// impl ColorAtlas {
//     pub fn save_to_png(&self, path: impl AsRef<Path>) {
//         use std::{fs::File, io::BufWriter, slice};
//         let file = File::create(path).unwrap();
//         let writer = BufWriter::new(file);
//         let size = self.size();
//         let mut encoder = png::Encoder::new(writer, size.width as u32, size.height as u32);
//         encoder.set_color(png::ColorType::Rgba);
//         encoder.set_depth(png::BitDepth::Eight);
//         let mut writer = encoder.write_header().unwrap();
//         let pixels = self.image.as_pixels();
//         let data = unsafe { slice::from_raw_parts(pixels.as_ptr() as *const u8, pixels.len() * 4) };
//         writer.write_image_data(&data).unwrap();
//     }
// }

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct GlyphImageKey {
    pub font_id: FontId,
    pub glyph_id: GlyphId,
    pub size: Size<usize>,
    pub kind: GlyphImageKind,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum GlyphImageKind {
    OutlineSdf,
    OutlineMsdf,
    Color,
}

