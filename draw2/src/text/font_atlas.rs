use {
    super::{
        font::{FontId, GlyphId},
        geom::{Point, Rect, Size},
        image::{Bgra, Image, Subimage, SubimageMut},
        num::Zero,
    },
    std::collections::HashMap,
};

#[derive(Clone, Debug)]
pub struct FontAtlas<T> {
    needs_reset: bool,
    image: Image<T>,
    dirty_rect: Rect<usize>,
    free_rects: Vec<Rect<usize>>,
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
            free_rects: vec![Rect::from(size)],
            cached_glyph_image_rects: HashMap::new(),
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

    pub unsafe fn replace_pixels(&mut self, pixels: Vec<T>) -> Vec<T> {
        self.image.replace_pixels(pixels)
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

    pub fn get_cached_glyph_image_rect(&self, key: &GlyphImageKey) -> Option<Rect<usize>> {
        self.cached_glyph_image_rects.get(key).copied()
    }

    pub fn get_cached_glyph_image_mut(&mut self, rect: Rect<usize>) -> SubimageMut<'_, T> {
        self.mark_dirty_rect(rect);
        self.image.subimage_mut(rect)
    }

    fn allocate_glyph_image(&mut self, size: Size<usize>) -> Option<Rect<usize>> {
        let rect = match self.place_rect(size) {
            Some(rect) => rect,
            None => {
                self.needs_reset = true;
                return None;
            }
        };
        self.mark_dirty_rect(rect);
        Some(rect)
    }

    fn mark_dirty_rect(&mut self, rect: Rect<usize>) {
        if self.dirty_rect.is_empty() {
            self.dirty_rect = rect;
        } else {
            self.dirty_rect = self.dirty_rect.union(rect);
        }
    }

    // Online max-rects packing: picks a stable placement for each incoming rectangle,
    // never relocates existing rectangles, and keeps free space fragmented efficiently.
    fn place_rect(&mut self, size: Size<usize>) -> Option<Rect<usize>> {
        if size.width == 0 || size.height == 0 {
            return None;
        }

        let mut best_index = None;
        let mut best_short = usize::MAX;
        let mut best_long = usize::MAX;
        let mut best_area = usize::MAX;
        for (index, free) in self.free_rects.iter().copied().enumerate() {
            if size.width > free.size.width || size.height > free.size.height {
                continue;
            }
            let leftover_w = free.size.width - size.width;
            let leftover_h = free.size.height - size.height;
            let short = leftover_w.min(leftover_h);
            let long = leftover_w.max(leftover_h);
            let area = free.size.width * free.size.height - size.width * size.height;
            if (short, long, area) < (best_short, best_long, best_area) {
                best_short = short;
                best_long = long;
                best_area = area;
                best_index = Some(index);
            }
        }

        let best_index = best_index?;
        let placed = Rect::new(self.free_rects[best_index].origin, size);
        self.split_free_rects(placed);
        self.prune_free_rects();
        Some(placed)
    }

    fn split_free_rects(&mut self, used: Rect<usize>) {
        let mut next_free_rects = Vec::with_capacity(self.free_rects.len().saturating_mul(2));
        for free in self.free_rects.drain(..) {
            if !rects_intersect(free, used) {
                next_free_rects.push(free);
                continue;
            }
            split_free_rect(free, used, &mut next_free_rects);
        }
        self.free_rects = next_free_rects;
    }

    fn prune_free_rects(&mut self) {
        let mut i = 0;
        while i < self.free_rects.len() {
            let mut remove_i = false;
            let mut j = i + 1;
            while j < self.free_rects.len() {
                if self.free_rects[i].contains_rect(self.free_rects[j]) {
                    self.free_rects.swap_remove(j);
                    continue;
                }
                if self.free_rects[j].contains_rect(self.free_rects[i]) {
                    remove_i = true;
                    break;
                }
                j += 1;
            }
            if remove_i {
                self.free_rects.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }

    pub fn reset_if_needed(&mut self) -> bool {
        if !self.needs_reset() {
            return false;
        }
        self.needs_reset = false;
        self.dirty_rect = Rect::ZERO;
        self.free_rects.clear();
        self.free_rects.push(Rect::from(self.size()));
        self.cached_glyph_image_rects.clear();
        true
    }
}

fn rects_intersect(a: Rect<usize>, b: Rect<usize>) -> bool {
    let a_max = a.max();
    let b_max = b.max();
    a.origin.x < b_max.x && a_max.x > b.origin.x && a.origin.y < b_max.y && a_max.y > b.origin.y
}

fn split_free_rect(free: Rect<usize>, used: Rect<usize>, out: &mut Vec<Rect<usize>>) {
    let free_max = free.max();
    let used_max = used.max();

    if used.origin.x < free_max.x && used_max.x > free.origin.x {
        if used.origin.y > free.origin.y {
            push_non_empty_rect(
                out,
                Rect::new(
                    free.origin,
                    Size::new(free.size.width, used.origin.y - free.origin.y),
                ),
            );
        }
        if used_max.y < free_max.y {
            push_non_empty_rect(
                out,
                Rect::new(
                    Point::new(free.origin.x, used_max.y),
                    Size::new(free.size.width, free_max.y - used_max.y),
                ),
            );
        }
    }

    if used.origin.y < free_max.y && used_max.y > free.origin.y {
        if used.origin.x > free.origin.x {
            push_non_empty_rect(
                out,
                Rect::new(
                    free.origin,
                    Size::new(used.origin.x - free.origin.x, free.size.height),
                ),
            );
        }
        if used_max.x < free_max.x {
            push_non_empty_rect(
                out,
                Rect::new(
                    Point::new(used_max.x, free.origin.y),
                    Size::new(free_max.x - used_max.x, free.size.height),
                ),
            );
        }
    }
}

fn push_non_empty_rect(out: &mut Vec<Rect<usize>>, rect: Rect<usize>) {
    if rect.size.width != 0 && rect.size.height != 0 {
        out.push(rect);
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

#[derive(Debug)]
pub enum GlyphImage<'a, T> {
    Cached(Rect<usize>),
    Allocated(SubimageMut<'a, T>),
}
