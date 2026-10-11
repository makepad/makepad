//! Shared storage for small, unmodified images drawn by the standard image shader.
//! Full or incompatible images keep their ordinary texture: capacity never
//! prevents an image from being displayed. Recorded draw calls pin each page.
use crate::*;
use std::collections::HashMap;

const SIDE: usize = 1024;
const MAX_SIDE: usize = 192;
const MAX_PAGES: usize = 4; // 16 MiB of texels, plus the retained CPU pixels.
const MAX_ENTRIES: usize = 4096;
const UPLOAD_BYTES_PER_DRAW: usize = 256 * 1024;

pub struct AtlasImage {
    pub texture: Texture,
    pub scale: Vec2f,
    pub pan: Vec2f,
}

#[derive(Clone, Copy)]
struct Entry {
    page: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

struct Page {
    texture: Texture,
    x: usize,
    y: usize,
    row_height: usize,
    last_draw: u64,
}

impl Page {
    fn place(&self, width: usize, height: usize) -> Option<(usize, usize, usize)> {
        let (x, y, row) = if self.x + width > SIDE {
            (0, self.y + self.row_height, 0)
        } else {
            (self.x, self.y, self.row_height)
        };
        (y + height <= SIDE).then_some((x, y, row.max(height)))
    }
}

#[derive(Default)]
struct ImageAtlas {
    pages: Vec<Page>,
    entries: HashMap<TextureId, Entry>,
    draw_id: u64,
    uploaded: usize,
    overflow_reported: bool,
}

/// Resolve an ordinary image binding to shared storage, when preserving its
/// filtering and lifetime is possible. No decoding, waiting or draw reordering.
/// `uv_size` is the portion of the source spanned by the drawn rectangle.
pub fn atlas_image(
    cx: &mut Cx2d,
    source: &Texture,
    drawn_size: Vec2d,
    uv_size: Vec2d,
) -> Option<AtlasImage> {
    if !source.can_atlas_image(cx) {
        return None;
    }
    let (width, height, mipmapped) = match source.format(cx) {
        TextureFormat::VecBGRAu8_32 {
            width,
            height,
            data: Some(data),
            ..
        } if data.len() >= width.saturating_mul(*height) => (*width, *height, false),
        TextureFormat::VecMipBGRAu8_32 {
            width,
            height,
            data: Some(data),
            wrap: TextureWrap::ClampToEdge,
            ..
        } if data.len() >= width.saturating_mul(*height) => (*width, *height, true),
        _ => return None,
    };
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    // A base-level atlas cannot replace a minified mip chain. Keep the source
    // sampler in that case, including an image whose size changes later.
    let dpi = cx.current_dpi_factor();
    if mipmapped
        && (drawn_size.x * dpi < width as f64 * uv_size.x
            || drawn_size.y * dpi < height as f64 * uv_size.y)
    {
        return None;
    }
    if !cx.has_global::<ImageAtlas>() {
        cx.set_global(ImageAtlas::default());
    }
    let mut cache = std::mem::take(cx.get_global::<ImageAtlas>());
    let result = cache.resolve(cx, source, width, height);
    *cx.get_global::<ImageAtlas>() = cache;
    if result.is_some() {
        source.mark_image_atlased(cx);
    }
    result
}

impl ImageAtlas {
    fn resolve(
        &mut self,
        cx: &mut Cx2d,
        source: &Texture,
        width: usize,
        height: usize,
    ) -> Option<AtlasImage> {
        if self.draw_id != cx.redraw_id {
            self.draw_id = cx.redraw_id;
            self.uploaded = 0;
        }
        let id = source.texture_id();
        if let Some(entry) = self.entries.get(&id).copied() {
            return Some(self.image(entry));
        }
        // New copies are preparation work. Existing pages still batch while a
        // finger is down; new images draw normally and may join on a later draw.
        if cx.fingers.any_areas_captured() {
            return None;
        }
        let bytes = (width + 2) * (height + 2) * 4;
        if self.uploaded + bytes > UPLOAD_BYTES_PER_DRAW {
            cx.redraw_all();
            return None;
        }
        let mut slot = self
            .pages
            .iter()
            .enumerate()
            .find_map(|(i, page)| page.place(width + 2, height + 2).map(|place| (i, place)));
        if slot.is_none() || self.entries.len() >= MAX_ENTRIES {
            // Never repack a page while any retained draw call can sample it.
            if let Some(i) = self
                .pages
                .iter()
                .position(|page| page.last_draw != self.draw_id && page.texture.readers() == 1)
            {
                self.entries.retain(|_, entry| entry.page != i);
                let page = &mut self.pages[i];
                // No retained call holds this page, but submitted GPU work
                // may still read it. Retire that allocation and install a new
                // handle before dropping the old one; unsupported retirement
                // backends must not reuse its still-live storage either.
                page.texture.release(cx);
                let data = page.texture.take_vec_u32(cx);
                page.texture = Texture::new_with_format(
                    cx,
                    TextureFormat::VecBGRAu8_32 {
                        width: SIDE,
                        height: SIDE,
                        data: Some(data),
                        updated: TextureUpdated::Full,
                    },
                );
                page.x = 0;
                page.y = 0;
                page.row_height = 0;
                slot = Some((i, page.place(width + 2, height + 2).unwrap()));
            }
        }
        if self.entries.len() >= MAX_ENTRIES {
            return self.full();
        }
        if slot.is_none() && self.pages.len() < MAX_PAGES {
            let texture = Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: SIDE,
                    height: SIDE,
                    data: Some(vec![0; SIDE * SIDE]),
                    updated: TextureUpdated::Full,
                },
            );
            let i = self.pages.len();
            self.pages.push(Page {
                texture,
                x: 0,
                y: 0,
                row_height: 0,
                last_draw: self.draw_id,
            });
            slot = Some((i, self.pages[i].place(width + 2, height + 2).unwrap()));
        }
        let Some((page_index, (x, y, row_height))) = slot else {
            return self.full();
        };
        let pixels = match source.format(cx) {
            TextureFormat::VecBGRAu8_32 {
                data: Some(data), ..
            }
            | TextureFormat::VecMipBGRAu8_32 {
                data: Some(data), ..
            } => data[..width * height].to_vec(),
            _ => return None,
        };
        let page = &mut self.pages[page_index];
        let mut data = page.texture.take_vec_u32(cx);
        for dy in 0..height + 2 {
            let row = dy.saturating_sub(1).min(height - 1) * width;
            let dest = (y + dy) * SIDE + x;
            data[dest] = pixels[row];
            data[dest + 1..dest + 1 + width].copy_from_slice(&pixels[row..row + width]);
            data[dest + width + 1] = pixels[row + width - 1];
        }
        page.texture.put_back_vec_u32(
            cx,
            data,
            Some(RectUsize::new(
                PointUsize::new(x, y),
                SizeUsize::new(width + 2, height + 2),
            )),
        );
        page.x = x + width + 2;
        page.y = y;
        page.row_height = row_height;
        self.uploaded += bytes;
        let entry = Entry {
            page: page_index,
            x: x + 1,
            y: y + 1,
            width,
            height,
        };
        self.entries.insert(id, entry);
        Some(self.image(entry))
    }

    fn image(&mut self, entry: Entry) -> AtlasImage {
        let page = &mut self.pages[entry.page];
        page.last_draw = self.draw_id;
        AtlasImage {
            texture: page.texture.clone(),
            scale: vec2(
                entry.width as f32 / SIDE as f32,
                entry.height as f32 / SIDE as f32,
            ),
            pan: vec2(entry.x as f32 / SIDE as f32, entry.y as f32 / SIDE as f32),
        }
    }

    fn full(&mut self) -> Option<AtlasImage> {
        if !self.overflow_reported {
            self.overflow_reported = true;
            crate::log!("Image atlas capacity reached; further images use individual textures");
        }
        None
    }
}
