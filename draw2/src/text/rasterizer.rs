use super::{
    font::{Font, GlyphId},
    font_atlas::{
        ColorAtlas, GlyphImageKey, GlyphImageKind, GrayscaleAtlas, MsdfAtlas,
    },
    geom::{Point, Rect, Size},
    glyph_outline::{Command, GlyphOutline},
    image::{Bgra, Image, R},
    msdfer,
    msdfer::Msdfer,
    sdfer,
    sdfer::Sdfer,
};
use std::collections::{HashMap, HashSet};
//use std::{fs::File, io::BufWriter, path::Path, slice};

#[derive(Debug)]
pub struct Rasterizer {
    sdfer: Sdfer,
    msdfer: Msdfer,
    msdf_resolution: MsdfResolutionSettings,
    msdf_complexity: MsdfComplexitySettings,
    outline_rasterization_mode: OutlineRasterizationMode,
    atlas: ColorAtlas,
    allocator: MultiPlaneAllocator,
    cached_slots: HashMap<GlyphImageKey, AtlasSlot>,
    outline_msdf_ready: HashSet<GlyphImageKey>,
    outline_msdf_pending: HashSet<GlyphImageKey>,
    outline_msdf_failed: HashSet<GlyphImageKey>,
    queued_msdf_jobs: Vec<QueuedMsdfJob>,
    atlas_epoch: u64,
}

impl Rasterizer {
    pub fn new(settings: Settings) -> Self {
        let atlas_size = Size::new(
            settings
                .grayscale_atlas_size
                .width
                .max(settings.color_atlas_size.width)
                .max(settings.msdf_atlas_size.width),
            settings
                .grayscale_atlas_size
                .height
                .max(settings.color_atlas_size.height)
                .max(settings.msdf_atlas_size.height),
        );
        Self {
            sdfer: Sdfer::new(settings.sdfer),
            msdfer: Msdfer::new(settings.msdfer),
            msdf_resolution: settings.msdf_resolution,
            msdf_complexity: settings.msdf_complexity,
            outline_rasterization_mode: settings.outline_rasterization_mode,
            atlas: ColorAtlas::new(atlas_size),
            allocator: MultiPlaneAllocator::new(atlas_size),
            cached_slots: HashMap::new(),
            outline_msdf_ready: HashSet::new(),
            outline_msdf_pending: HashSet::new(),
            outline_msdf_failed: HashSet::new(),
            queued_msdf_jobs: Vec::new(),
            atlas_epoch: 0,
        }
    }

    pub fn sdfer(&self) -> &Sdfer {
        &self.sdfer
    }

    pub fn msdfer(&self) -> &Msdfer {
        &self.msdfer
    }

    pub fn msdf_resolution(&self) -> MsdfResolutionSettings {
        self.msdf_resolution
    }

    pub fn msdf_complexity(&self) -> MsdfComplexitySettings {
        self.msdf_complexity
    }

    pub fn outline_rasterization_mode(&self) -> OutlineRasterizationMode {
        self.outline_rasterization_mode
    }

    pub fn set_outline_rasterization_mode(&mut self, mode: OutlineRasterizationMode) {
        self.outline_rasterization_mode = mode;
    }

    pub fn on_atlas_reset(&mut self) {
        self.allocator.reset(self.atlas.size());
        self.cached_slots.clear();
        self.outline_msdf_ready.clear();
        self.outline_msdf_pending.clear();
        self.outline_msdf_failed.clear();
        self.queued_msdf_jobs.clear();
        self.atlas_epoch = self.atlas_epoch.wrapping_add(1);
    }

    pub fn take_queued_msdf_jobs(&mut self) -> Vec<QueuedMsdfJob> {
        std::mem::take(&mut self.queued_msdf_jobs)
    }

    pub fn apply_completed_msdf_job(&mut self, job: CompletedMsdfJob) {
        if job.epoch != self.atlas_epoch {
            return;
        }
        if !self.outline_msdf_pending.remove(&job.key) {
            return;
        }
        let Some(slot) = self.cached_slots.get(&job.key).copied() else {
            return;
        };
        if job.pixels.len() != job.key.size.width.saturating_mul(job.key.size.height) {
            self.outline_msdf_failed.insert(job.key);
            return;
        }

        {
            let mut dst = self.atlas.get_cached_glyph_image_mut(slot.rect);
            for y in 0..job.key.size.height {
                for x in 0..job.key.size.width {
                    let point = Point::new(x, y);
                    let old = dst[point];
                    let msdf = job.pixels[y * job.key.size.width + x];
                    // Keep alpha as seeded SDF coverage for stable visual parity while RGB carries MSDF.
                    dst[point] = Bgra::new(msdf.b(), msdf.g(), msdf.r(), old.a());
                }
            }
        }
        self.outline_msdf_ready.insert(job.key);
    }

    pub fn grayscale_atlas(&self) -> &GrayscaleAtlas {
        &self.atlas
    }

    pub fn color_atlas(&self) -> &ColorAtlas {
        &self.atlas
    }

    pub fn msdf_atlas(&self) -> &MsdfAtlas {
        &self.atlas
    }

    pub fn grayscale_atlas_mut(&mut self) -> &mut GrayscaleAtlas {
        &mut self.atlas
    }

    pub fn color_atlas_mut(&mut self) -> &mut ColorAtlas {
        &mut self.atlas
    }

    pub fn msdf_atlas_mut(&mut self) -> &mut MsdfAtlas {
        &mut self.atlas
    }

    fn get_cached_slot(&self, key: &GlyphImageKey) -> Option<AtlasSlot> {
        self.cached_slots.get(key).copied()
    }

    fn allocate_sdf_slot(&mut self, key: GlyphImageKey) -> Option<(AtlasSlot, bool)> {
        if let Some(slot) = self.get_cached_slot(&key) {
            return Some((slot, false));
        }
        let Some(slot) = self.allocator.allocate_sdf_slot(key.size) else {
            self.atlas.request_reset();
            return None;
        };
        self.cached_slots.insert(key, slot);
        Some((slot, true))
    }

    fn allocate_shared_slot(&mut self, key: GlyphImageKey) -> Option<(AtlasSlot, bool)> {
        if let Some(slot) = self.get_cached_slot(&key) {
            return Some((slot, false));
        }
        let Some(rect) = self.allocator.allocate_shared_slot(key.size) else {
            self.atlas.request_reset();
            return None;
        };
        let slot = AtlasSlot {
            rect,
            plane: AtlasPlane::R,
        };
        self.cached_slots.insert(key, slot);
        Some((slot, true))
    }

    fn seed_msdf_slot_from_sdf(&mut self, dst_slot: AtlasSlot, sdf_glyph: RasterizedGlyph) {
        if sdf_glyph.atlas_kind != AtlasKind::Grayscale {
            return;
        }
        let src_rect = sdf_glyph.atlas_image_bounds;
        if src_rect.size != dst_slot.rect.size {
            return;
        }
        let src_plane = AtlasPlane::from_index(sdf_glyph.atlas_plane as usize);
        let mut src_values = Vec::with_capacity(src_rect.size.width * src_rect.size.height);
        {
            let atlas = self.atlas.image();
            for y in 0..src_rect.size.height {
                for x in 0..src_rect.size.width {
                    let src = atlas[src_rect.origin + Size::new(x, y)];
                    src_values.push(src_plane.get(src));
                }
            }
        }
        let mut dst = self.atlas.get_cached_glyph_image_mut(dst_slot.rect);
        for y in 0..dst_slot.rect.size.height {
            for x in 0..dst_slot.rect.size.width {
                let v = src_values[y * dst_slot.rect.size.width + x];
                dst[Point::new(x, y)] = Bgra::new(v, v, v, v);
            }
        }
    }

    pub fn rasterize_glyph(
        &mut self,
        font: &Font,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
    ) -> Option<RasterizedGlyph> {
        if let Some(rasterized_glyph) = self.rasterize_glyph_outline(font, glyph_id, dpxs_per_em) {
            return Some(rasterized_glyph);
        };
        if let Some(rasterized_glyph) =
            self.rasterize_glyph_raster_image(font, glyph_id, dpxs_per_em)
        {
            return Some(rasterized_glyph);
        }
        None
    }
    /*
    pub fn save_to_png(item:&Image<R>, path: impl AsRef<Path>) {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let size = item.size();
        let mut encoder = png::Encoder::new(writer, size.width as u32, size.height as u32);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let pixels = item.as_pixels();
        let data = unsafe { slice::from_raw_parts(pixels.as_ptr() as *const u8, pixels.len()) };
        writer.write_image_data(&data).unwrap();
    }
    */
    fn rasterize_glyph_outline(
        &mut self,
        font: &Font,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
    ) -> Option<RasterizedGlyph> {
        match self.outline_rasterization_mode {
            OutlineRasterizationMode::Sdf => {
                self.rasterize_glyph_outline_sdf(font, glyph_id, dpxs_per_em)
            }
            OutlineRasterizationMode::Msdf => {
                self.rasterize_glyph_outline_msdf(font, glyph_id, dpxs_per_em)
            }
        }
    }

    fn rasterize_glyph_outline_sdf(
        &mut self,
        font: &Font,
        glyph_id: GlyphId,
        mut dpxs_per_em: f32,
    ) -> Option<RasterizedGlyph> {
        debug_assert_eq!(
            self.sdfer.settings().padding,
            self.msdfer.settings().padding
        );
        dpxs_per_em = dpxs_per_em.max(self.msdf_resolution.min_dpxs_per_em);
        let mut outline = None;
        let bounds_in_ems = font.glyph_outline_bounds_in_ems(glyph_id, &mut outline)?;
        let outline = outline.unwrap_or_else(|| font.glyph_outline(glyph_id).unwrap());
        let atlas_image_size = glyph_outline_image_size(bounds_in_ems.size, dpxs_per_em);
        let atlas_image_padding = self.sdfer.settings().padding;
        let key = GlyphImageKey {
            font_id: font.id(),
            glyph_id,
            size: atlas_image_size + Size::from(atlas_image_padding) * 2,
            kind: GlyphImageKind::OutlineSdf,
        };
        let (slot, allocated) = self.allocate_sdf_slot(key.clone())?;
        let atlas_image_bounds = if !allocated {
            slot.rect
        } else {
            let mut image = self.atlas.get_cached_glyph_image_mut(slot.rect);
            {
                let mut coverage = Image::new(atlas_image_size);
                outline.rasterize(
                    dpxs_per_em,
                    &mut coverage.subimage_mut(atlas_image_size.into()),
                );
                let sdf_image_size = atlas_image_size + Size::from(atlas_image_padding) * 2;
                let mut sdf_image = Image::<R>::new(sdf_image_size);
                self.sdfer.coverage_to_sdf(
                    &coverage.subimage(atlas_image_size.into()),
                    &mut sdf_image.subimage_mut(Rect::from(sdf_image_size)),
                );
                for y in 0..sdf_image_size.height {
                    for x in 0..sdf_image_size.width {
                        let v = sdf_image[Point::new(x, y)].r();
                        let point = Point::new(x, y);
                        let old = image[point];
                        image[point] = slot.plane.set(old, v);
                    }
                }
            }
            slot.rect
        };

        return Some(RasterizedGlyph {
            atlas_kind: AtlasKind::Grayscale,
            atlas_size: self.atlas.size(),
            atlas_image_bounds,
            atlas_image_padding,
            atlas_plane: slot.plane.index(),
            origin_in_dpxs: bounds_in_ems.origin * dpxs_per_em,
            dpxs_per_em,
        });
    }

    fn rasterize_glyph_outline_msdf(
        &mut self,
        font: &Font,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
    ) -> Option<RasterizedGlyph> {
        // Always keep small text on SDF, even if an MSDF slot already exists.
        if dpxs_per_em <= self.msdf_resolution.min_request_dpxs_per_em {
            return self.rasterize_glyph_outline_sdf(font, glyph_id, dpxs_per_em);
        }
        let mut outline = None;
        let bounds_in_ems = font.glyph_outline_bounds_in_ems(glyph_id, &mut outline)?;
        let outline = outline.unwrap_or_else(|| font.glyph_outline(glyph_id).unwrap());
        let complexity = estimate_outline_complexity(&outline);
        if !is_msdf_complexity_acceptable(self.msdf_complexity, complexity) {
            return self.rasterize_glyph_outline_sdf(font, glyph_id, dpxs_per_em);
        }
        let dpxs_per_em = dpxs_per_em.max(self.msdf_resolution.min_dpxs_per_em);
        let atlas_image_size = glyph_outline_image_size(bounds_in_ems.size, dpxs_per_em);
        let atlas_image_padding = self.msdfer.settings().padding;
        let key = GlyphImageKey {
            font_id: font.id(),
            glyph_id,
            size: atlas_image_size + Size::from(atlas_image_padding) * 2,
            kind: GlyphImageKind::OutlineMsdf,
        };
        if self.outline_msdf_failed.contains(&key) {
            return self.rasterize_glyph_outline_sdf(font, glyph_id, dpxs_per_em);
        }
        if self.outline_msdf_ready.contains(&key) {
            if let Some(slot) = self.get_cached_slot(&key) {
                return Some(RasterizedGlyph {
                    atlas_kind: AtlasKind::Msdf,
                    atlas_size: self.atlas.size(),
                    atlas_image_bounds: slot.rect,
                    atlas_image_padding,
                    atlas_plane: AtlasPlane::R.index(),
                    origin_in_dpxs: bounds_in_ems.origin * dpxs_per_em,
                    dpxs_per_em,
                });
            }
            self.outline_msdf_ready.remove(&key);
        }

        let sdf_glyph = self.rasterize_glyph_outline_sdf(font, glyph_id, dpxs_per_em)?;
        if self.outline_msdf_ready.contains(&key) || self.outline_msdf_pending.contains(&key) {
            return Some(sdf_glyph);
        }
        let (slot, allocated) = match self.allocate_shared_slot(key.clone()) {
            Some(slot) => slot,
            None => return Some(sdf_glyph),
        };
        if allocated {
            self.seed_msdf_slot_from_sdf(slot, sdf_glyph);
        }
        if self.outline_msdf_pending.insert(key.clone()) {
            self.queued_msdf_jobs.push(QueuedMsdfJob {
                key,
                outline,
                dpxs_per_em,
                epoch: self.atlas_epoch,
            });
        }
        Some(sdf_glyph)
    }

    fn rasterize_glyph_raster_image(
        &mut self,
        font: &Font,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
    ) -> Option<RasterizedGlyph> {
        const PADDING: usize = 2;

        let raster_image = font.glyph_raster_image(glyph_id, dpxs_per_em)?;
        let key = GlyphImageKey {
            font_id: font.id(),
            glyph_id,
            size: raster_image.decode_size() + Size::from(2 * PADDING),
            kind: GlyphImageKind::Color,
        };
        let (slot, allocated) = self.allocate_shared_slot(key)?;
        let atlas_image_bounds = if !allocated {
            slot.rect
        } else {
            let mut image = self.atlas.get_cached_glyph_image_mut(slot.rect);
            {
                let size = image.size();
                image = image.subimage_mut(Rect::from(size).unpad(PADDING));
                raster_image.decode(&mut image);
            }
            slot.rect
        };
        return Some(RasterizedGlyph {
            atlas_kind: AtlasKind::Color,
            atlas_size: self.atlas.size(),
            atlas_image_bounds,
            atlas_image_padding: PADDING,
            atlas_plane: AtlasPlane::R.index(),
            origin_in_dpxs: raster_image.origin_in_dpxs(),
            dpxs_per_em: raster_image.dpxs_per_em(),
        });
    }
}

#[derive(Clone, Copy, Debug)]
struct AtlasSlot {
    rect: Rect<usize>,
    plane: AtlasPlane,
}

#[derive(Clone, Copy, Debug)]
enum AtlasPlane {
    R,
    G,
    B,
    A,
}

impl AtlasPlane {
    fn from_index(index: usize) -> Self {
        match index & 3 {
            0 => Self::R,
            1 => Self::G,
            2 => Self::B,
            _ => Self::A,
        }
    }

    fn index(self) -> u8 {
        match self {
            Self::R => 0,
            Self::G => 1,
            Self::B => 2,
            Self::A => 3,
        }
    }

    fn set(self, pixel: Bgra, value: u8) -> Bgra {
        match self {
            Self::R => Bgra::new(pixel.b(), pixel.g(), value, pixel.a()),
            Self::G => Bgra::new(pixel.b(), value, pixel.r(), pixel.a()),
            Self::B => Bgra::new(value, pixel.g(), pixel.r(), pixel.a()),
            Self::A => Bgra::new(pixel.b(), pixel.g(), pixel.r(), value),
        }
    }

    fn get(self, pixel: Bgra) -> u8 {
        match self {
            Self::R => pixel.r(),
            Self::G => pixel.g(),
            Self::B => pixel.b(),
            Self::A => pixel.a(),
        }
    }
}

#[derive(Debug)]
struct MultiPlaneAllocator {
    planes: [RectPacker; 4],
    plane_used_area: [usize; 4],
    round_robin_cursor: usize,
}

impl MultiPlaneAllocator {
    fn new(size: Size<usize>) -> Self {
        Self {
            planes: std::array::from_fn(|_| RectPacker::new(size)),
            plane_used_area: [0; 4],
            round_robin_cursor: 0,
        }
    }

    fn reset(&mut self, size: Size<usize>) {
        self.planes = std::array::from_fn(|_| RectPacker::new(size));
        self.plane_used_area = [0; 4];
        self.round_robin_cursor = 0;
    }

    fn allocate_sdf_slot(&mut self, size: Size<usize>) -> Option<AtlasSlot> {
        let mut best_plane = None;
        let mut best_rank = (usize::MAX, usize::MAX, usize::MAX, usize::MAX, usize::MAX);
        for offset in 0..4 {
            let plane = (self.round_robin_cursor + offset) & 3;
            let Some(score) = self.planes[plane].peek_best_fit(size) else {
                continue;
            };
            // Keep single-channel occupancy balanced while still using best-fit packing.
            let rank = (
                self.plane_used_area[plane],
                offset,
                score.short,
                score.long,
                score.area,
            );
            if rank < best_rank {
                best_rank = rank;
                best_plane = Some(plane);
            }
        }
        let plane = best_plane?;
        let rect = self.planes[plane].allocate(size)?;
        let area = size.width.saturating_mul(size.height);
        self.plane_used_area[plane] = self.plane_used_area[plane].saturating_add(area);
        self.round_robin_cursor = (plane + 1) & 3;
        Some(AtlasSlot {
            rect,
            plane: AtlasPlane::from_index(plane),
        })
    }

    fn allocate_shared_slot(&mut self, size: Size<usize>) -> Option<Rect<usize>> {
        if size.width == 0 || size.height == 0 {
            return None;
        }
        let mut candidates = self.planes[0].free_rects().to_vec();
        for plane in 1..4 {
            let mut next = Vec::new();
            for a in candidates.iter().copied() {
                for b in self.planes[plane].free_rects().iter().copied() {
                    let Some(common) = rect_intersection(a, b) else {
                        continue;
                    };
                    if common.size.width >= size.width && common.size.height >= size.height {
                        next.push(common);
                    }
                }
            }
            prune_contained_rects(&mut next);
            if next.is_empty() {
                return None;
            }
            candidates = next;
        }

        let origin = choose_best_fit_origin(&candidates, size)?;
        let placed = Rect::new(origin, size);
        for plane in &mut self.planes {
            if !plane.reserve(placed) {
                return None;
            }
        }
        let area = size.width.saturating_mul(size.height);
        for used in &mut self.plane_used_area {
            *used = used.saturating_add(area);
        }
        Some(placed)
    }
}

#[derive(Clone, Copy, Debug)]
struct PlacementScore {
    origin: Point<usize>,
    short: usize,
    long: usize,
    area: usize,
}

#[derive(Clone, Debug)]
struct RectPacker {
    size: Size<usize>,
    free_rects: Vec<Rect<usize>>,
}

impl RectPacker {
    fn new(size: Size<usize>) -> Self {
        Self {
            size,
            free_rects: vec![Rect::from(size)],
        }
    }

    fn free_rects(&self) -> &[Rect<usize>] {
        &self.free_rects
    }

    fn peek_best_fit(&self, size: Size<usize>) -> Option<PlacementScore> {
        let mut best: Option<PlacementScore> = None;
        let mut best_rank = (usize::MAX, usize::MAX, usize::MAX);
        for free in self.free_rects.iter().copied() {
            if size.width > free.size.width || size.height > free.size.height {
                continue;
            }
            let leftover_w = free.size.width - size.width;
            let leftover_h = free.size.height - size.height;
            let short = leftover_w.min(leftover_h);
            let long = leftover_w.max(leftover_h);
            let area = free.size.width * free.size.height - size.width * size.height;
            let rank = (short, long, area);
            if rank < best_rank {
                best_rank = rank;
                best = Some(PlacementScore {
                    origin: free.origin,
                    short,
                    long,
                    area,
                });
            }
        }
        best
    }

    fn allocate(&mut self, size: Size<usize>) -> Option<Rect<usize>> {
        let score = self.peek_best_fit(size)?;
        let rect = Rect::new(score.origin, size);
        if self.reserve(rect) {
            Some(rect)
        } else {
            None
        }
    }

    fn reserve(&mut self, used: Rect<usize>) -> bool {
        if used.size.width == 0 || used.size.height == 0 {
            return false;
        }
        if !Rect::from(self.size).contains_rect(used) {
            return false;
        }
        if !self
            .free_rects
            .iter()
            .copied()
            .any(|free| free.contains_rect(used))
        {
            return false;
        }
        let mut next_free_rects = Vec::with_capacity(self.free_rects.len().saturating_mul(2));
        for free in self.free_rects.drain(..) {
            if !rects_intersect(free, used) {
                next_free_rects.push(free);
                continue;
            }
            split_free_rect(free, used, &mut next_free_rects);
        }
        self.free_rects = next_free_rects;
        prune_contained_rects(&mut self.free_rects);
        true
    }
}

fn choose_best_fit_origin(rects: &[Rect<usize>], size: Size<usize>) -> Option<Point<usize>> {
    let mut best_origin = None;
    let mut best_rank = (usize::MAX, usize::MAX, usize::MAX);
    for rect in rects.iter().copied() {
        if size.width > rect.size.width || size.height > rect.size.height {
            continue;
        }
        let leftover_w = rect.size.width - size.width;
        let leftover_h = rect.size.height - size.height;
        let short = leftover_w.min(leftover_h);
        let long = leftover_w.max(leftover_h);
        let area = rect.size.width * rect.size.height - size.width * size.height;
        let rank = (short, long, area);
        if rank < best_rank {
            best_rank = rank;
            best_origin = Some(rect.origin);
        }
    }
    best_origin
}

fn prune_contained_rects(rects: &mut Vec<Rect<usize>>) {
    let mut i = 0;
    while i < rects.len() {
        let mut remove_i = false;
        let mut j = i + 1;
        while j < rects.len() {
            if rects[i].contains_rect(rects[j]) {
                rects.swap_remove(j);
                continue;
            }
            if rects[j].contains_rect(rects[i]) {
                remove_i = true;
                break;
            }
            j += 1;
        }
        if remove_i {
            rects.swap_remove(i);
        } else {
            i += 1;
        }
    }
}

fn rect_intersection(a: Rect<usize>, b: Rect<usize>) -> Option<Rect<usize>> {
    let min_x = a.origin.x.max(b.origin.x);
    let min_y = a.origin.y.max(b.origin.y);
    let a_max = a.max();
    let b_max = b.max();
    let max_x = a_max.x.min(b_max.x);
    let max_y = a_max.y.min(b_max.y);
    if max_x <= min_x || max_y <= min_y {
        return None;
    }
    Some(Rect::new(
        Point::new(min_x, min_y),
        Size::new(max_x - min_x, max_y - min_y),
    ))
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

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub sdfer: sdfer::Settings,
    pub msdfer: msdfer::Settings,
    pub msdf_resolution: MsdfResolutionSettings,
    pub msdf_complexity: MsdfComplexitySettings,
    pub outline_rasterization_mode: OutlineRasterizationMode,
    pub grayscale_atlas_size: Size<usize>,
    pub color_atlas_size: Size<usize>,
    pub msdf_atlas_size: Size<usize>,
}

#[derive(Clone, Debug)]
pub struct QueuedMsdfJob {
    pub key: GlyphImageKey,
    pub outline: GlyphOutline,
    pub dpxs_per_em: f32,
    pub epoch: u64,
}

#[derive(Clone, Debug)]
pub struct CompletedMsdfJob {
    pub key: GlyphImageKey,
    pub pixels: Vec<Bgra>,
    pub epoch: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct MsdfResolutionSettings {
    pub min_request_dpxs_per_em: f32,
    pub min_dpxs_per_em: f32,
    pub base_dpxs_per_em: f32,
    pub max_dpxs_per_em: f32,
    pub target_feature_texels: f32,
    pub dpx_quantum: f32,
    pub min_feature_floor_ems: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct MsdfComplexitySettings {
    pub max_outline_commands: usize,
    pub max_estimated_segments: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct RasterizedGlyph {
    pub atlas_kind: AtlasKind,
    pub atlas_size: Size<usize>,
    pub atlas_image_bounds: Rect<usize>,
    pub atlas_image_padding: usize,
    pub atlas_plane: u8,
    pub origin_in_dpxs: Point<f32>,
    pub dpxs_per_em: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AtlasKind {
    Grayscale,
    Color,
    Msdf,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OutlineRasterizationMode {
    Sdf,
    Msdf,
}

fn glyph_outline_image_size(size_in_ems: Size<f32>, dpxs_per_em: f32) -> Size<usize> {
    let size_in_dpxs = size_in_ems * dpxs_per_em;
    Size::new(
        size_in_dpxs.width.ceil() as usize,
        size_in_dpxs.height.ceil() as usize,
    )
}

#[derive(Clone, Copy, Debug)]
struct OutlineComplexity {
    outline_commands: usize,
    estimated_segments: usize,
}

fn estimate_outline_complexity(outline: &GlyphOutline) -> OutlineComplexity {
    const QUAD_COMPLEXITY_SEGMENTS: usize = 8;
    const CUBIC_COMPLEXITY_SEGMENTS: usize = 12;

    let mut estimated_segments = 0usize;
    for command in outline.commands().iter().copied() {
        match command {
            Command::MoveTo(_) => {}
            Command::LineTo(_) => estimated_segments = estimated_segments.saturating_add(1),
            Command::QuadTo(_, _) => {
                estimated_segments = estimated_segments.saturating_add(QUAD_COMPLEXITY_SEGMENTS);
            }
            Command::CurveTo(_, _, _) => {
                estimated_segments = estimated_segments.saturating_add(CUBIC_COMPLEXITY_SEGMENTS);
            }
            Command::Close => estimated_segments = estimated_segments.saturating_add(1),
        }
    }

    OutlineComplexity {
        outline_commands: outline.commands().len(),
        estimated_segments,
    }
}

fn is_msdf_complexity_acceptable(
    settings: MsdfComplexitySettings,
    complexity: OutlineComplexity,
) -> bool {
    complexity.outline_commands <= settings.max_outline_commands
        && complexity.estimated_segments <= settings.max_estimated_segments
}

fn choose_msdf_resolution(
    settings: MsdfResolutionSettings,
    outline: &GlyphOutline,
    requested_dpxs_per_em: f32,
) -> f32 {
    let min_dpxs = settings.min_dpxs_per_em.max(1.0);
    let base_dpxs = settings.base_dpxs_per_em.max(min_dpxs);
    let max_dpxs = settings.max_dpxs_per_em.max(base_dpxs);
    let base = if requested_dpxs_per_em < min_dpxs {
        min_dpxs
    } else {
        base_dpxs
    };

    let min_feature = estimate_outline_min_feature_ems(outline);
    let effective_min_feature = min_feature
        .unwrap_or(settings.min_feature_floor_ems)
        .max(settings.min_feature_floor_ems);
    let required = (settings.target_feature_texels.max(0.5) / effective_min_feature).max(base);

    let mut selected = required.min(max_dpxs);
    selected = quantize_up(selected, settings.dpx_quantum.max(0.0));
    selected = selected.clamp(base, max_dpxs);
    selected
}

fn estimate_outline_min_feature_ems(outline: &GlyphOutline) -> Option<f32> {
    const QUAD_STEPS: usize = 8;
    const CUBIC_STEPS: usize = 12;
    const EPS: f32 = 0.000_000_1;

    let transform = outline.rasterize_transform(1.0);
    let mut min_feature = f32::INFINITY;
    let mut first = None;
    let mut last = None;

    for command in outline.commands().iter().copied() {
        match command {
            Command::MoveTo(p) => {
                let p = p.apply_transform(transform);
                first = Some(p);
                last = Some(p);
            }
            Command::LineTo(p1) => {
                if let Some(p0) = last {
                    let p1 = p1.apply_transform(transform);
                    update_min_feature(&mut min_feature, p0, p1, EPS);
                    last = Some(p1);
                }
            }
            Command::QuadTo(p1, p2) => {
                if let Some(p0) = last {
                    let p1 = p1.apply_transform(transform);
                    let p2 = p2.apply_transform(transform);
                    let mut prev = p0;
                    for step in 1..=QUAD_STEPS {
                        let t = step as f32 / QUAD_STEPS as f32;
                        let next = quadratic_point(p0, p1, p2, t);
                        update_min_feature(&mut min_feature, prev, next, EPS);
                        prev = next;
                    }
                    last = Some(p2);
                }
            }
            Command::CurveTo(p1, p2, p3) => {
                if let Some(p0) = last {
                    let p1 = p1.apply_transform(transform);
                    let p2 = p2.apply_transform(transform);
                    let p3 = p3.apply_transform(transform);
                    let mut prev = p0;
                    for step in 1..=CUBIC_STEPS {
                        let t = step as f32 / CUBIC_STEPS as f32;
                        let next = cubic_point(p0, p1, p2, p3, t);
                        update_min_feature(&mut min_feature, prev, next, EPS);
                        prev = next;
                    }
                    last = Some(p3);
                }
            }
            Command::Close => {
                if let (Some(p0), Some(p1)) = (last, first) {
                    update_min_feature(&mut min_feature, p0, p1, EPS);
                }
                last = first;
            }
        }
    }

    if min_feature.is_finite() {
        Some(min_feature)
    } else {
        None
    }
}

fn update_min_feature(min_feature: &mut f32, p0: Point<f32>, p1: Point<f32>, eps: f32) {
    let dx = p1.x - p0.x;
    let dy = p1.y - p0.y;
    let d = (dx * dx + dy * dy).sqrt();
    if d > eps && d < *min_feature {
        *min_feature = d;
    }
}

fn quantize_up(value: f32, quantum: f32) -> f32 {
    if quantum <= 0.0 {
        return value;
    }
    (value / quantum).ceil() * quantum
}

fn quadratic_point(p0: Point<f32>, p1: Point<f32>, p2: Point<f32>, t: f32) -> Point<f32> {
    let omt = 1.0 - t;
    Point::new(
        omt * omt * p0.x + 2.0 * omt * t * p1.x + t * t * p2.x,
        omt * omt * p0.y + 2.0 * omt * t * p1.y + t * t * p2.y,
    )
}

fn cubic_point(
    p0: Point<f32>,
    p1: Point<f32>,
    p2: Point<f32>,
    p3: Point<f32>,
    t: f32,
) -> Point<f32> {
    let omt = 1.0 - t;
    let omt2 = omt * omt;
    let t2 = t * t;
    Point::new(
        omt2 * omt * p0.x + 3.0 * omt2 * t * p1.x + 3.0 * omt * t2 * p2.x + t2 * t * p3.x,
        omt2 * omt * p0.y + 3.0 * omt2 * t * p1.y + 3.0 * omt * t2 * p2.y + t2 * t * p3.y,
    )
}

/*
use {
    crate::{
        font_atlas::CxFontAtlas,
        font_loader::{FontId, FontLoader},
        sdf_glyph_rasterizer::SdfGlyphRasterizer,
        svg_glyph_rasterizer::SvgGlyphRasterizer,
    },
    makepad_platform::*,
    std::{
        collections::HashMap,
        fs::{File, OpenOptions},
        io::{self, Read, Write},
        path::Path,
    },
};

#[derive(Debug)]
pub struct GlyphRasterizer {
    sdf_glyph_rasterizer: SdfGlyphRasterizer,
    svg_glyph_rasterizer: SvgGlyphRasterizer,
    cache: Cache,
}

impl GlyphRasterizer {
    pub fn new(cache_dir: Option<&Path>) -> Self {
        Self {
            sdf_glyph_rasterizer: SdfGlyphRasterizer::new(),
            svg_glyph_rasterizer: SvgGlyphRasterizer::new(),
            cache: Cache::new(cache_dir).expect("failed to load glyph raster cache"),
        }
    }

    pub fn get_or_rasterize_glyph(
        &mut self,
        font_loader: &mut FontLoader,
        font_atlas: &mut CxFontAtlas,
        Command {
            mode,
            params:
                params @ Params {
                    font_id,
                    atlas_page_id,
                    glyph_id,
                },
            ..
        }: Command,
    ) -> RasterizedGlyph<'_> {
        let font = font_loader[font_id].as_mut().unwrap();
        let atlas_page = &font.atlas_pages[atlas_page_id];
        let font_size = atlas_page.font_size_in_device_pixels;
        let font_path = font_loader.path(font_id).unwrap();
        let key = CacheKey::new(&font_path, glyph_id, font_size);
        if !self.cache.contains_key(&key) {
            self.cache
                .insert_with(key, |output| match mode {
                    Mode::Sdf => self.sdf_glyph_rasterizer.rasterize_sdf_glyph(
                        font_loader,
                        font_atlas,
                        params,
                        output,
                    ),
                    Mode::Svg => self.svg_glyph_rasterizer.rasterize_svg_glyph(
                        font_loader,
                        font_atlas,
                        params,
                        output,
                    ),
                })
                .expect("failed to update glyph raster cache")
        }
        self.cache.get(key).unwrap()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Command {
    pub mode: Mode,
    pub params: Params,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Mode {
    Svg,
    Sdf,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Params {
    pub font_id: FontId,
    pub atlas_page_id: usize,
    pub glyph_id: usize,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RasterizedGlyph<'a> {
    pub size: SizeUsize,
    pub bytes: &'a [u8],
}

#[derive(Debug)]
struct Cache {
    data: Vec<u8>,
    data_file: Option<File>,
    index: HashMap<CacheKey, CacheIndexEntry>,
    index_file: Option<File>,
}

impl Cache {
    fn new(dir: Option<&Path>) -> io::Result<Self> {
        let mut data_file = match dir {
            Some(dir) => Some(
                OpenOptions::new()
                    .create(true)
                    .read(true)
                    .write(true)
                    .open(dir.join("glyph_raster_data"))?,
            ),
            None => None,
        };

        let mut data = Vec::new();
        if let Some(data_file) = &mut data_file {
            data_file.read_to_end(&mut data)?;
        }

        let mut index_file = match dir {
            Some(dir) => Some(
                OpenOptions::new()
                    .create(true)
                    .read(true)
                    .write(true)
                    .open(dir.join("glyph_raster_index"))?,
            ),
            None => None,
        };

        let mut index = HashMap::new();
        if let Some(index_file) = &mut index_file {
            loop {
                let mut buffer = [0; 32];
                match index_file.read_exact(&mut buffer) {
                    Ok(_) => (),
                    Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
                    Err(error) => return Err(error),
                }
                index.insert(
                    CacheKey::from_bytes(buffer[0..8].try_into().unwrap()),
                    CacheIndexEntry::from_bytes(buffer[8..32].try_into().unwrap()),
                );
            }
        }
        Ok(Self {
            data,
            data_file,
            index,
            index_file,
        })
    }

    fn contains_key(&self, key: &CacheKey) -> bool {
        self.index.contains_key(key)
    }

    fn get(&self, key: CacheKey) -> Option<RasterizedGlyph<'_>> {
        let CacheIndexEntry { size, offset, len } = self.index.get(&key).copied()?;
        Some(RasterizedGlyph {
            size,
            bytes: &self.data[offset..][..len],
        })
    }

    fn insert_with(
        &mut self,
        key: CacheKey,
        f: impl FnOnce(&mut Vec<u8>) -> SizeUsize,
    ) -> io::Result<()> {
        let offset = self.data.len();
        let size = f(&mut self.data);
        let len = self.data.len() - offset;
        if let Some(data_file) = &mut self.data_file {
            data_file.write_all(&self.data[offset..][..len])?;
        }
        let index_entry = CacheIndexEntry { size, offset, len };
        self.index.insert(key, index_entry);
        if let Some(index_file) = &mut self.index_file {
            let mut buffer = [0; 32];
            buffer[0..8].copy_from_slice(&key.to_bytes());
            buffer[8..32].copy_from_slice(&index_entry.to_bytes());
            index_file.write_all(&buffer)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct CacheKey(LiveId);

impl CacheKey {
    fn new(font_path: &str, glyph_id: usize, font_size: f64) -> Self {
        Self(
            LiveId::empty()
                .bytes_append(font_path.as_bytes())
                .bytes_append(&glyph_id.to_ne_bytes())
                .bytes_append(&font_size.to_ne_bytes()),
        )
    }

    fn from_bytes(bytes: [u8; 8]) -> Self {
        Self(LiveId(u64::from_be_bytes(bytes)))
    }

    fn to_bytes(self) -> [u8; 8] {
        self.0 .0.to_be_bytes()
    }
}

#[derive(Clone, Copy, Debug)]
struct CacheIndexEntry {
    size: SizeUsize,
    offset: usize,
    len: usize,
}

impl CacheIndexEntry {
    fn from_bytes(bytes: [u8; 24]) -> Self {
        Self {
            size: SizeUsize {
                width: u32::from_be_bytes(bytes[0..4].try_into().unwrap())
                    .try_into()
                    .unwrap(),
                height: u32::from_be_bytes(bytes[4..8].try_into().unwrap())
                    .try_into()
                    .unwrap(),
            },
            offset: u64::from_be_bytes(bytes[8..16].try_into().unwrap())
                .try_into()
                .unwrap(),
            len: u64::from_be_bytes(bytes[16..24].try_into().unwrap())
                .try_into()
                .unwrap(),
        }
    }

    fn to_bytes(self) -> [u8; 24] {
        let mut bytes = [0; 24];
        bytes[0..4].copy_from_slice(&u32::try_from(self.size.width).unwrap().to_be_bytes());
        bytes[4..8].copy_from_slice(&u32::try_from(self.size.height).unwrap().to_be_bytes());
        bytes[8..16].copy_from_slice(&u64::try_from(self.offset).unwrap().to_be_bytes());
        bytes[16..24].copy_from_slice(&u64::try_from(self.len).unwrap().to_be_bytes());
        bytes
    }
}
*/
