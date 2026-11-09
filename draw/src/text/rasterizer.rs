use super::{
    font::{Font, FontId, GlyphId},
    font_atlas::{ColorAtlas, GlyphImageKey, GrayscaleAtlas, MsdfAtlas},
    geom::{Point, Size},
    glyph_outline::GlyphOutline,
    image::{Bgra, Image, R},
    msdfer::Msdfer,
    sdfer::Sdfer,
};
use fxhash::{FxHashMap, FxHashSet};
pub use makepad_font_core::raster::{
    AtlasKind, MsdfComplexitySettings, MsdfResolutionSettings, OutlineRasterizationMode,
    RasterizedGlyph, Settings,
};
use makepad_font_core::raster::{
    plan_color, plan_outline_msdf, plan_outline_sdf, AtlasPlane, AtlasSlot, MultiPlaneAllocator,
    SdfPlan, COLOR_PADDING,
};
use std::rc::Rc;
//use std::{fs::File, io::BufWriter, path::Path, slice};

#[derive(Debug)]
pub struct Rasterizer {
    sdfer: Sdfer,
    msdfer: Msdfer,
    settings: Settings,
    atlas: ColorAtlas,
    allocator: MultiPlaneAllocator,
    cached_slots: FxHashMap<GlyphImageKey, AtlasSlot>,
    outline_msdf_ready: FxHashSet<GlyphImageKey>,
    outline_msdf_pending: FxHashSet<GlyphImageKey>,
    outline_msdf_failed: FxHashSet<GlyphImageKey>,
    queued_msdf_jobs: Vec<QueuedOutlineJob>,
    outline_sdf_pending: FxHashSet<GlyphImageKey>,
    queued_sdf_jobs: Vec<QueuedOutlineJob>,
    /// The fonts and sizes (as `f32` bits) whose printable ASCII we've already queued.
    prewarm_sdf_sizes: FxHashSet<(FontId, u32)>,
    /// The fonts and sizes whose printable ASCII we queue after this frame's own glyphs.
    queued_sdf_prewarms: Vec<(Rc<Font>, f32)>,
    atlas_epoch: u64,
}

impl Rasterizer {
    pub fn epoch(&self) -> u64 {
        self.atlas_epoch
    }

    /// A kept glyph as it draws now: this rasteriser owns its atlas, a kept glyph is what it was.
    pub fn refresh_glyph(&mut self, glyph: RasterizedGlyph) -> RasterizedGlyph {
        glyph
    }

    /// As `refresh_glyph`, for slug curves.
    pub fn refresh_slug_glyph(&mut self, glyph: super::slug_atlas::SlugGlyphInfo) -> super::slug_atlas::SlugGlyphInfo {
        glyph
    }

    /// Glyphs here never leave the atlas while drawn: nothing to record per draw list.
    pub fn take_glyph_uses(&mut self, _into: &mut Vec<[u32; 3]>) {}

    pub(crate) fn export_glyph(&self, glyph: RasterizedGlyph) -> Option<PublishedGlyph> {
        let key = self.cached_slots.iter().find_map(|(key, slot)| {
            (slot.rect == glyph.atlas_image_bounds && slot.plane.index() == glyph.atlas_plane)
                .then(|| key.clone())
        })?;
        let mut pixels = Vec::with_capacity(key.size.width * key.size.height);
        for y in 0..key.size.height {
            for x in 0..key.size.width {
                pixels.push(self.atlas.image()[glyph.atlas_image_bounds.origin + Size::new(x, y)]);
            }
        }
        Some(PublishedGlyph { key, glyph, pixels })
    }

    /// Install worker-rasterized pixels; this path never loads an outline or shapes text.
    pub(crate) fn import_glyph(&mut self, published: &PublishedGlyph) -> Option<RasterizedGlyph> {
        let (slot, fresh) = if published.glyph.atlas_kind == AtlasKind::Grayscale {
            self.allocate_sdf_slot(published.key.clone())?
        } else {
            self.allocate_shared_slot(published.key.clone())?
        };
        if fresh {
            let mut image = self.atlas.get_cached_glyph_image_mut(slot.rect);
            let src_plane = AtlasPlane::from_index(published.glyph.atlas_plane as usize);
            for y in 0..slot.rect.size.height {
                for x in 0..slot.rect.size.width {
                    let at = Point::new(x, y);
                    let pixel = published.pixels[y * slot.rect.size.width + x];
                    image[at] = if published.glyph.atlas_kind == AtlasKind::Grayscale {
                        slot.plane.set(image[at], src_plane.get(pixel))
                    } else {
                        pixel
                    };
                }
            }
        }
        Some(RasterizedGlyph {
            source: Default::default(),
            atlas_size: self.atlas.size(),
            atlas_image_bounds: slot.rect,
            atlas_plane: slot.plane.index(),
            ..published.glyph
        })
    }

    pub fn new(settings: Settings) -> Self {
        let atlas_size = settings.atlas_size;
        Self {
            sdfer: Sdfer::new(settings.sdfer),
            msdfer: Msdfer::new(settings.msdfer),
            settings,
            atlas: ColorAtlas::new_zeroed(atlas_size),
            allocator: MultiPlaneAllocator::new(atlas_size),
            cached_slots: FxHashMap::default(),
            outline_msdf_ready: FxHashSet::default(),
            outline_msdf_pending: FxHashSet::default(),
            outline_msdf_failed: FxHashSet::default(),
            queued_msdf_jobs: Vec::new(),
            outline_sdf_pending: FxHashSet::default(),
            queued_sdf_jobs: Vec::new(),
            prewarm_sdf_sizes: FxHashSet::default(),
            queued_sdf_prewarms: Vec::new(),
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
        self.settings.msdf_resolution
    }

    pub fn msdf_complexity(&self) -> MsdfComplexitySettings {
        self.settings.msdf_complexity
    }

    pub fn outline_rasterization_mode(&self) -> OutlineRasterizationMode {
        self.settings.outline_rasterization_mode
    }

    pub fn set_outline_rasterization_mode(&mut self, mode: OutlineRasterizationMode) {
        self.settings.outline_rasterization_mode = mode;
    }

    pub fn on_atlas_reset(&mut self) {
        self.allocator.reset(self.atlas.size());
        self.cached_slots.clear();
        self.outline_msdf_ready.clear();
        self.outline_msdf_pending.clear();
        self.outline_msdf_failed.clear();
        self.queued_msdf_jobs.clear();
        self.outline_sdf_pending.clear();
        self.queued_sdf_jobs.clear();
        self.prewarm_sdf_sizes.clear();
        self.queued_sdf_prewarms.clear();
        self.atlas_epoch = self.atlas_epoch.wrapping_add(1);
    }

    pub fn queued_outline_job_count(&self) -> usize {
        self.queued_msdf_jobs.len() + self.queued_sdf_jobs.len()
    }

    pub fn take_queued_msdf_jobs(&mut self) -> Vec<QueuedOutlineJob> {
        std::mem::take(&mut self.queued_msdf_jobs)
    }

    /// Takes the SDF jobs queued since the last call, with the printable ASCII we prewarm last.
    pub fn take_queued_sdf_jobs(&mut self) -> Vec<QueuedOutlineJob> {
        for (font, dpxs_per_em) in std::mem::take(&mut self.queued_sdf_prewarms) {
            let glyph_ids = font.with_ttf_parser_face(|face| {
                (' '..='~')
                    .filter_map(|ch| face.glyph_index(ch))
                    .collect::<Vec<_>>()
            });
            for glyph_id in glyph_ids {
                if let Some(plan) = plan_outline_sdf(&font, glyph_id.0, dpxs_per_em, &self.settings) {
                    self.queue_glyph_outline_sdf(&plan);
                }
            }
        }
        std::mem::take(&mut self.queued_sdf_jobs)
    }

    pub fn apply_completed_outline_job(&mut self, job: CompletedOutlineJob) {
        match job {
            CompletedOutlineJob::Msdf(job) => self.apply_completed_msdf_job(job),
            CompletedOutlineJob::Sdf(job) => self.apply_completed_sdf_job(job),
        }
    }

    pub fn apply_completed_sdf_job(&mut self, job: CompletedSdfJob) {
        if job.epoch != self.atlas_epoch || !self.outline_sdf_pending.remove(&job.key) {
            return;
        }
        let Some((slot, allocated)) = self.allocate_sdf_slot(job.key) else {
            return;
        };
        if allocated {
            self.write_sdf_image(slot, &job.sdf_image);
        }
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
        let _phase = makepad_platform::thread::ui_phase(makepad_platform::thread::UiPhase::FontAtlas);
        if let Some(rasterized_glyph) =
            self.rasterize_glyph_raster_image(font, glyph_id, dpxs_per_em)
        {
            return Some(rasterized_glyph);
        }
        if let Some(rasterized_glyph) = self.rasterize_glyph_outline(font, glyph_id, dpxs_per_em) {
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
        match self.settings.outline_rasterization_mode {
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
        dpxs_per_em: f32,
    ) -> Option<RasterizedGlyph> {
        let plan = plan_outline_sdf(font, glyph_id, dpxs_per_em, &self.settings)?;
        let (slot, allocated) = self.allocate_sdf_slot(plan.key.clone())?;
        if allocated {
            let sdf_image = plan.render(&mut self.sdfer);
            self.write_sdf_image(slot, &sdf_image);
        }
        Some(self.build_sdf_glyph(slot, &plan))
    }

    /// Returns the glyph's SDF if the atlas has it, or else queues it for the worker.
    ///
    /// The first time a font misses at some size, we also queue that font's printable ASCII.
    pub fn get_or_queue_glyph_outline_sdf(
        &mut self,
        font: &Rc<Font>,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
    ) -> Option<RasterizedGlyph> {
        let plan = plan_outline_sdf(font, glyph_id, dpxs_per_em, &self.settings)?;
        if let Some(slot) = self.get_cached_slot(&plan.key) {
            return Some(self.build_sdf_glyph(slot, &plan));
        }
        self.queue_glyph_outline_sdf(&plan);
        if self.prewarm_sdf_sizes.insert((font.id(), plan.dpxs_per_em.to_bits())) {
            self.queued_sdf_prewarms.push((font.clone(), plan.dpxs_per_em));
        }
        None
    }

    fn queue_glyph_outline_sdf(&mut self, plan: &SdfPlan) {
        if self.cached_slots.contains_key(&plan.key) || !self.outline_sdf_pending.insert(plan.key.clone()) {
            return;
        }
        self.queued_sdf_jobs.push(QueuedOutlineJob {
            key: plan.key.clone(),
            outline: plan.outline(),
            dpxs_per_em: plan.dpxs_per_em,
            epoch: self.atlas_epoch,
        });
    }

    fn write_sdf_image(&mut self, slot: AtlasSlot, sdf_image: &Image<R>) {
        let mut image = self.atlas.get_cached_glyph_image_mut(slot.rect);
        for y in 0..slot.rect.size.height {
            for x in 0..slot.rect.size.width {
                let point = Point::new(x, y);
                image[point] = slot.plane.set(image[point], sdf_image[point].r());
            }
        }
    }

    fn build_sdf_glyph(&self, slot: AtlasSlot, plan: &SdfPlan) -> RasterizedGlyph {
        RasterizedGlyph {
            source: Default::default(),
            atlas_kind: AtlasKind::Grayscale,
            atlas_size: self.atlas.size(),
            atlas_image_bounds: slot.rect,
            atlas_image_padding: plan.padding,
            atlas_plane: slot.plane.index(),
            origin_in_dpxs: plan.origin_in_dpxs,
            dpxs_per_em: plan.dpxs_per_em,
        }
    }

    fn rasterize_glyph_outline_msdf(
        &mut self,
        font: &Font,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
    ) -> Option<RasterizedGlyph> {
        // Small text and outlines too complex for the MSDF generator stay on SDF.
        let Some(plan) = plan_outline_msdf(font, glyph_id, dpxs_per_em, &self.settings) else {
            return self.rasterize_glyph_outline_sdf(font, glyph_id, dpxs_per_em);
        };
        let key = plan.key.clone();
        let dpxs_per_em = plan.dpxs_per_em;
        if self.outline_msdf_failed.contains(&key) {
            return self.rasterize_glyph_outline_sdf(font, glyph_id, dpxs_per_em);
        }
        if self.outline_msdf_ready.contains(&key) {
            if let Some(slot) = self.get_cached_slot(&key) {
                return Some(RasterizedGlyph {
            source: Default::default(),
                    atlas_kind: AtlasKind::Msdf,
                    atlas_size: self.atlas.size(),
                    atlas_image_bounds: slot.rect,
                    atlas_image_padding: plan.padding,
                    atlas_plane: AtlasPlane::R.index(),
                    origin_in_dpxs: plan.origin_in_dpxs,
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
            // The job crosses a thread boundary, so it needs an owned copy of the
            // outline. This deep clone happens at most once per glyph per atlas
            // epoch, guarded by the pending set above.
            self.queued_msdf_jobs.push(QueuedOutlineJob {
                key,
                outline: plan.outline(),
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
        let plan = plan_color(font, glyph_id, dpxs_per_em)?;
        let (slot, allocated) = self.allocate_shared_slot(plan.key.clone())?;
        if allocated {
            plan.render(font, glyph_id, dpxs_per_em, self.atlas.get_cached_glyph_image_mut(slot.rect));
        }
        Some(RasterizedGlyph {
            source: Default::default(),
            atlas_kind: AtlasKind::Color,
            atlas_size: self.atlas.size(),
            atlas_image_bounds: slot.rect,
            atlas_image_padding: COLOR_PADDING,
            atlas_plane: AtlasPlane::R.index(),
            origin_in_dpxs: plan.origin_in_dpxs,
            dpxs_per_em: plan.dpxs_per_em,
        })
    }
}

/// A glyph outline for the worker to build an SDF or MSDF image of, depending on its key's kind.
#[derive(Clone, Debug)]
pub struct QueuedOutlineJob {
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

#[derive(Clone, Debug)]
pub struct CompletedSdfJob {
    pub key: GlyphImageKey,
    pub sdf_image: Image<R>,
    pub epoch: u64,
}

#[derive(Clone, Debug)]
pub enum CompletedOutlineJob {
    Msdf(CompletedMsdfJob),
    Sdf(CompletedSdfJob),
}

#[derive(Debug)]
pub(crate) struct PublishedGlyph {
    key: GlyphImageKey,
    glyph: RasterizedGlyph,
    pixels: Vec<Bgra>,
}
