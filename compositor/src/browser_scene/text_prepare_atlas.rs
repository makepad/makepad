use super::*;

pub(super) struct BrowserGlyphAtlasPage {
    pub(super) id: BrowserGlyphAtlasPageId,
    pub(super) generation: u64,
    pub(super) format: BrowserGlyphFormat,
    pub(super) size: Size<usize>,
    pub(super) texture: Texture,
    pixels: Vec<Bgra>,
    allocator: ShelfAllocator,
    dirty_rect: Option<TextRect<usize>>,
}

impl BrowserGlyphAtlasPage {
    pub(super) fn new(
        cx: &mut Cx,
        id: BrowserGlyphAtlasPageId,
        generation: u64,
        format: BrowserGlyphFormat,
        size: Size<usize>,
    ) -> Self {
        Self {
            id,
            generation,
            format,
            size,
            texture: Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: size.width,
                    height: size.height,
                    data: None,
                    updated: TextureUpdated::Empty,
                },
            ),
            pixels: vec![Bgra::default(); size.width.saturating_mul(size.height)],
            allocator: ShelfAllocator::new(size),
            dirty_rect: None,
        }
    }

    pub(super) fn page_ref(&self) -> BrowserAtlasPageRef {
        BrowserAtlasPageRef {
            format: self.format,
            page_id: self.id,
            page_generation: self.generation,
        }
    }

    pub(super) fn allocate(&mut self, size: Size<usize>) -> Option<TextRect<usize>> {
        self.allocator.allocate(size)
    }

    fn mark_dirty_rect(&mut self, rect: TextRect<usize>) {
        self.dirty_rect = Some(match self.dirty_rect {
            Some(old) => old.union(rect),
            None => rect,
        });
    }

    fn write_pixel(&mut self, x: usize, y: usize, pixel: Bgra) {
        let index = y.saturating_mul(self.size.width).saturating_add(x);
        if let Some(dst) = self.pixels.get_mut(index) {
            *dst = pixel;
        }
    }

    pub(super) fn upload_if_dirty(&mut self, cx: &mut Cx) {
        let Some(dirty_rect) = self.dirty_rect.take() else {
            return;
        };
        self.texture.put_back_vec_u32(
            cx,
            self.pixels.iter().map(|pixel| pixel.bits).collect(),
            Some(RectUsize::new(
                PointUsize::new(dirty_rect.origin.x, dirty_rect.origin.y),
                SizeUsize::new(dirty_rect.size.width, dirty_rect.size.height),
            )),
        );
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ShelfAllocator {
    size: Size<usize>,
    cursor_x: usize,
    cursor_y: usize,
    shelf_height: usize,
}

impl ShelfAllocator {
    pub(super) fn new(size: Size<usize>) -> Self {
        Self {
            size,
            cursor_x: 0,
            cursor_y: 0,
            shelf_height: 0,
        }
    }

    pub(super) fn allocate(&mut self, size: Size<usize>) -> Option<TextRect<usize>> {
        if size.width == 0 || size.height == 0 {
            return None;
        }
        if size.width > self.size.width || size.height > self.size.height {
            return None;
        }
        if self.cursor_x + size.width > self.size.width {
            self.cursor_x = 0;
            self.cursor_y = self.cursor_y.saturating_add(self.shelf_height);
            self.shelf_height = 0;
        }
        if self.cursor_y + size.height > self.size.height {
            return None;
        }
        let rect = TextRect::new(Point::new(self.cursor_x, self.cursor_y), size);
        self.cursor_x = self.cursor_x.saturating_add(size.width);
        self.shelf_height = self.shelf_height.max(size.height);
        Some(rect)
    }
}

pub(super) fn blit_alpha_image(
    page: &mut BrowserGlyphAtlasPage,
    rect: TextRect<usize>,
    image: &Image<R>,
) {
    page.mark_dirty_rect(rect);
    for y in 0..rect.size.height {
        for x in 0..rect.size.width {
            let value = image[Point::new(x, y)].r();
            page.write_pixel(rect.origin.x + x, rect.origin.y + y, Bgra::new(0, 0, 0, value));
        }
    }
}

pub(super) fn blit_bgra_image(
    page: &mut BrowserGlyphAtlasPage,
    rect: TextRect<usize>,
    image: &Image<Bgra>,
) {
    page.mark_dirty_rect(rect);
    for y in 0..rect.size.height {
        for x in 0..rect.size.width {
            page.write_pixel(rect.origin.x + x, rect.origin.y + y, image[Point::new(x, y)]);
        }
    }
}

pub(super) fn blit_bgra_pixels(
    page: &mut BrowserGlyphAtlasPage,
    rect: TextRect<usize>,
    pixels: &[Bgra],
    size: Size<usize>,
) {
    page.mark_dirty_rect(rect);
    for y in 0..size.height {
        for x in 0..size.width {
            let index = y.saturating_mul(size.width).saturating_add(x);
            if let Some(pixel) = pixels.get(index).copied() {
                page.write_pixel(rect.origin.x + x, rect.origin.y + y, pixel);
            }
        }
    }
}
