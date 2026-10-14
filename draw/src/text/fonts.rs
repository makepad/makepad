#[cfg(target_arch = "wasm32")]
use super::{
    image::{Bgra, Image},
    msdfer::Msdfer,
    rasterizer::{CompletedMsdfJob, QueuedMsdfJob},
    slug_atlas::SlugAtlas,
};
use {
    super::{
        font::{Font, FontId, GlyphId},
        font_family::{FontFamily, FontFamilyId},
        layouter::{self, LaidoutText, LayoutParams, Layouter},
        loader::{FontDefinition, FontFamilyDefinition},
        rasterizer::{OutlineRasterizationMode, Rasterizer},
        slug_atlas::SlugGlyphCacheResult,
    },
    crate::makepad_platform::*,
    fxhash::FxHashSet,
    std::{cell::RefCell, rc::Rc},
};
#[cfg(target_arch = "wasm32")]
use std::mem::ManuallyDrop;

#[derive(Default)]
struct LazyFontRequests {
    requested: FxHashSet<(FontFamilyId, LazyFontFamily)>,
}

impl LazyFontRequests {
    fn take_for_glyph_miss(
        &mut self,
        family_id: FontFamilyId,
        lazy_family: LazyFontFamily,
        text: &str,
        has_missing_glyph: bool,
    ) -> bool {
        has_missing_glyph
            && text.chars().any(|ch| lazy_family.contains(ch))
            && self.requested.insert((family_id, lazy_family))
    }

    fn contains(&self, family_id: FontFamilyId, lazy_family: LazyFontFamily) -> bool {
        self.requested.contains(&(family_id, lazy_family))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FontsMemoryBytes {
    pub atlas_bytes: usize,
    pub layout_cache_bytes: usize,
}

pub struct Fonts {
    layouter: Layouter,
    lazy_font_requests: LazyFontRequests,
    missing_font_paths: FxHashSet<String>,
    #[cfg(target_arch = "wasm32")]
    needs_prepare_atlases: bool,
    /// The glyph atlas. Under the HAL it is the host's texture, lent to this session.
    atlas_texture: Texture,
    #[cfg(not(target_arch = "wasm32"))]
    slug_curve_texture: Texture,
    #[cfg(not(target_arch = "wasm32"))]
    slug_band_texture: Texture,
    #[cfg(target_arch = "wasm32")]
    slug_atlas: SlugAtlas,
    /// Outline glyphs at or above this size draw as slug curves, smaller ones from the
    /// SDF/MSDF atlas. 0 (the default, every platform): all text is slug.
    slug_min_dpxs_per_em: f32,
    #[cfg(target_arch = "wasm32")]
    msdf_job_sender: FromUISender<QueuedMsdfJob>,
    #[cfg(target_arch = "wasm32")]
    msdf_result_receiver: ToUIReceiver<CompletedMsdfJob>,
    /// Upgrades the MSDF worker has sent (it raises the UI signal after each) and those
    /// applied at a draw: the signal hook redraws while sent > applied.
    #[cfg(target_arch = "wasm32")]
    msdf_sent: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(target_arch = "wasm32")]
    msdf_applied: usize,
    /// Glyph rasters sent to the MSDF worker and not applied yet.
    #[cfg(target_arch = "wasm32")]
    msdf_jobs_in_flight: usize,
    /// Fonts whose resource this process could not read (see
    /// [`Self::note_font_unavailable`]); their families render without them
    /// until the resource registry moves on (`unavailable_generation`).
    unavailable_fonts: FxHashSet<FontId>,
    /// The `Cx::script_resource_generation` the set was built under.
    unavailable_generation: u64,
}

impl Fonts {
    /// Under the HAL the atlases are the host's: textures lent to this session when it answers
    /// (`prepare_textures`), glyphs asked for as they are drawn.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(cx: &mut Cx, settings: layouter::Settings) -> Self {
        let layouter = Layouter::new(settings);
        Self {
            layouter,
            lazy_font_requests: Default::default(),
            missing_font_paths: FxHashSet::default(),
            // Before AtlasInfo arrives, the unresolved slot is this transparent texel. The
            // lent host atlas reserves the same texel for the lifetime of its texture.
            atlas_texture: Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: 1,
                    height: 1,
                    data: Some(vec![0]),
                    updated: TextureUpdated::Full,
                },
            ),
            slug_curve_texture: Texture::new_with_format(cx, TextureFormat::Unknown),
            // the curve scan needs no band data: a texel stands in
            slug_band_texture: Texture::new_with_format(
                cx,
                TextureFormat::VecRGBAf32 { width: 1, height: 1, data: None, updated: TextureUpdated::Empty },
            ),
            slug_min_dpxs_per_em: 0.0,
            unavailable_fonts: FxHashSet::default(),
            unavailable_generation: 0,
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn new(cx: &mut Cx, settings: layouter::Settings) -> Self {
        let layouter = Layouter::new(settings);
        let (atlas_size, msdfer_settings) = {
            let rasterizer = layouter.rasterizer().borrow();
            (rasterizer.color_atlas().size(), rasterizer.msdfer().settings())
        };

        let mut msdf_job_sender: FromUISender<QueuedMsdfJob> = Default::default();
        let msdf_result_receiver: ToUIReceiver<CompletedMsdfJob> = Default::default();
        let worker_rx = msdf_job_sender
            .receiver()
            .expect("MSDF worker receiver is taken exactly once");
        let worker_tx = msdf_result_receiver.sender();
        let msdf_sent = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let worker_sent = msdf_sent.clone();
        if let Ok(task) = cx.spawn_worker(move || {
            let mut msdfer = Msdfer::new(msdfer_settings);
            while let Ok(job) = worker_rx.recv() {
                let mut msdf = Image::<Bgra>::new(job.key.size);
                msdfer.outline_to_msdf(
                    &job.outline,
                    job.dpxs_per_em,
                    &mut msdf.subimage_mut(super::geom::Rect::from(job.key.size)),
                );
                // counted before the send: its UI signal must find the count raised
                worker_sent.fetch_add(1, std::sync::atomic::Ordering::Release);
                if worker_tx
                    .send(CompletedMsdfJob {
                        key: job.key,
                        pixels: msdf.into_pixels(),
                        epoch: job.epoch,
                    })
                    .is_err()
                {
                    break;
                }
            }
        }) {
            task.detach();
        }

        Self {
            layouter,
            lazy_font_requests: Default::default(),
            missing_font_paths: FxHashSet::default(),
            needs_prepare_atlases: false,
            atlas_texture: Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: atlas_size.width,
                    height: atlas_size.height,
                    data: None,
                    updated: TextureUpdated::Empty,
                },
            ),
            slug_atlas: SlugAtlas::new(cx),
            slug_min_dpxs_per_em: 0.0,
            msdf_job_sender,
            msdf_result_receiver,
            msdf_sent,
            msdf_applied: 0,
            msdf_jobs_in_flight: 0,
            unavailable_fonts: FxHashSet::default(),
            unavailable_generation: 0,
        }
    }

    pub fn rasterizer(&self) -> &Rc<RefCell<Rasterizer>> {
        self.layouter.rasterizer()
    }

    pub fn outline_rasterization_mode(&self) -> OutlineRasterizationMode {
        self.layouter
            .rasterizer()
            .borrow()
            .outline_rasterization_mode()
    }

    pub fn set_outline_rasterization_mode(&mut self, mode: OutlineRasterizationMode) {
        self.layouter
            .rasterizer()
            .borrow_mut()
            .set_outline_rasterization_mode(mode);
    }

    pub fn grayscale_texture(&self) -> &Texture {
        &self.atlas_texture
    }

    pub fn color_texture(&self) -> &Texture {
        &self.atlas_texture
    }

    pub fn msdf_texture(&self) -> &Texture {
        &self.atlas_texture
    }

    pub fn slug_curve_texture(&self) -> &Texture {
        #[cfg(not(target_arch = "wasm32"))]
        return &self.slug_curve_texture;
        #[cfg(target_arch = "wasm32")]
        self.slug_atlas.curve_texture()
    }

    pub fn slug_band_texture(&self) -> &Texture {
        #[cfg(not(target_arch = "wasm32"))]
        return &self.slug_band_texture;
        #[cfg(target_arch = "wasm32")]
        self.slug_atlas.band_texture()
    }

    pub fn should_use_slug_glyph(&self, dpxs_per_em: f32) -> bool {
        dpxs_per_em >= self.slug_min_dpxs_per_em
    }

    /// Selects the SDF/MSDF atlas for outline glyphs below `dpxs_per_em` (`f32::INFINITY`: the
    /// atlas at every size; `outline_rasterization_mode` picks SDF or MSDF). Slug is the default
    /// at every size (0).
    pub fn set_slug_min_dpxs_per_em(&mut self, dpxs_per_em: f32) {
        self.slug_min_dpxs_per_em = dpxs_per_em.max(0.0);
    }

    pub fn slug_min_dpxs_per_em(&self) -> f32 {
        self.slug_min_dpxs_per_em
    }

    /// The slug curves of a glyph, from the host's curve texture. Asked for on the first look; until
    /// they arrive its usual rectangle draws the zero-curve slot (the text redraws on arrival).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn get_or_cache_slug_glyph(
        &mut self,
        font: &Font,
        glyph_id: GlyphId,
    ) -> SlugGlyphCacheResult {
        use super::rasterizer::SlugLookup;
        let mut rasterizer = self.layouter.rasterizer().borrow_mut();
        match rasterizer.client_mut().slug(font, glyph_id) {
            SlugLookup::Ready(info) => SlugGlyphCacheResult::Ready(info),
            SlugLookup::Empty => SlugGlyphCacheResult::Unavailable,
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn get_or_cache_slug_glyph(&mut self, font: &Font, glyph_id: GlyphId) -> SlugGlyphCacheResult {
        self.slug_atlas.get_or_cache_glyph(font, glyph_id)
    }

    /// Glyph rasters queued for or running on the MSDF worker: text that
    /// draws with a fallback until they land (`Cx::pending_async_loads`).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn pending_glyph_rasters(&self) -> usize {
        self.layouter.rasterizer().try_borrow().map_or(0, |rasterizer| rasterizer.client().pending())
    }

    #[cfg(target_arch = "wasm32")]
    pub fn pending_glyph_rasters(&self) -> usize {
        let queued = self
            .layouter
            .rasterizer()
            .try_borrow()
            .map_or(0, |rasterizer| rasterizer.queued_msdf_job_count());
        self.msdf_jobs_in_flight + queued
    }

    /// Under the HAL the host uploads the curves: nothing here waits for an upload.
    pub fn slug_cache_generation(&self) -> u64 {
        #[cfg(not(target_arch = "wasm32"))]
        return 0;
        #[cfg(target_arch = "wasm32")]
        self.slug_atlas.cache_generation()
    }

    pub fn slug_uploaded_generation(&self) -> u64 {
        #[cfg(not(target_arch = "wasm32"))]
        return 0;
        #[cfg(target_arch = "wasm32")]
        self.slug_atlas.uploaded_generation()
    }

    /// Uploads any newly appended SLUG curve/band data immediately so draw calls
    /// in the current frame can see glyphs cached during the draw loop.
    /// CPU bytes the text system holds: the glyph atlas pixels while the
    /// atlas (not its texture) owns them, and the laid-out text cache. Font
    /// file bytes are script resources and are counted by `Cx::memory_report`.
    pub fn memory_bytes(&self) -> FontsMemoryBytes {
        // under the HAL the atlas is the host's (its ledger counts it), not this session's
        #[cfg(not(target_arch = "wasm32"))]
        let atlas_bytes = 0;
        #[cfg(target_arch = "wasm32")]
        let atlas_bytes = self
            .layouter
            .rasterizer()
            .borrow()
            .color_atlas()
            .image()
            .as_pixels()
            .len()
            .saturating_mul(4);
        FontsMemoryBytes { atlas_bytes, layout_cache_bytes: self.layouter.cache_bytes() }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn flush_slug_textures(&mut self, cx: &mut Cx) -> bool {
        self.slug_atlas.prepare_textures(cx)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn flush_slug_textures(&mut self, _cx: &mut Cx) -> bool {
        false
    }

    pub fn is_font_family_known(&self, id: FontFamilyId) -> bool {
        self.layouter.is_font_family_known(id)
    }

    pub(crate) fn lazy_font_is_requested(
        &self,
        id: FontFamilyId,
        family: LazyFontFamily,
    ) -> bool {
        self.lazy_font_requests.contains(id, family)
    }

    pub(crate) fn take_lazy_font_request(
        &mut self,
        id: FontFamilyId,
        family: LazyFontFamily,
        text: &str,
        has_missing_glyph: bool,
    ) -> bool {
        self.lazy_font_requests
            .take_for_glyph_miss(id, family, text, has_missing_glyph)
    }

    pub fn is_font_family_complete(
        &self,
        id: FontFamilyId,
        expected_member_count: usize,
    ) -> bool {
        self.layouter
            .loader
            .font_family_definitions
            .get(&id)
            .map(|def| {
                def.expected_member_count == expected_member_count
                    && def.font_ids.len() == expected_member_count
            })
            .unwrap_or(false)
    }

    /// Records that `id`'s resource could not be read; true the first time,
    /// so the caller can say so once. The family drops the member instead
    /// of asking for it again every frame.
    pub fn note_font_unavailable(&mut self, id: FontId) -> bool {
        self.unavailable_fonts.insert(id)
    }

    /// A resource was registered or loaded since the unavailable set was
    /// built: forget it, so the missing members are asked for once more
    /// (a font file provisioned later, a resource registered again).
    pub fn sync_unavailable_fonts(&mut self, resource_generation: u64) {
        if self.unavailable_generation != resource_generation {
            self.unavailable_generation = resource_generation;
            self.unavailable_fonts.clear();
        }
    }

    pub fn is_font_unavailable(&self, id: FontId) -> bool {
        self.unavailable_fonts.contains(&id)
    }

    pub fn is_font_known(&self, id: FontId) -> bool {
        self.layouter.is_font_known(id)
    }

    /// The loaded font with this id (loading it from its definition when needed).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn font(&mut self, id: FontId) -> Option<Rc<Font>> {
        self.layouter.is_font_known(id).then(|| self.layouter.loader.get_or_load_font(id).clone())
    }

    /// True the first time a resource path fails to load, so the error logs once.
    pub fn note_missing_font(&mut self, resource_path: &str) -> bool {
        self.missing_font_paths.insert(resource_path.to_string())
    }

    pub fn define_font_family(&mut self, id: FontFamilyId, definition: FontFamilyDefinition) {
        self.layouter.define_font_family(id, definition);
    }

    /// See [`layouter::Layouter::family_generation`].
    pub fn family_generation(&self) -> u64 {
        self.layouter.family_generation()
    }

    pub fn set_font_family_definition(
        &mut self,
        id: FontFamilyId,
        definition: FontFamilyDefinition,
    ) {
        self.layouter.set_font_family_definition(id, definition);
    }

    pub fn define_font(&mut self, id: FontId, definition: FontDefinition) {
        self.layouter.define_font(id, definition);
    }

    pub fn get_or_load_font_family(&mut self, id: FontFamilyId) -> Rc<FontFamily> {
        self.layouter.get_or_load_font_family(id)
    }

    pub fn get_or_layout(&mut self, params: impl LayoutParams) -> Rc<LaidoutText> {
        self.layouter.get_or_layout(params)
    }

    /// See [`layouter::Layouter::record_glyphs`].
    pub fn record_glyphs(&mut self, on: bool) {
        self.layouter.record_glyphs(on)
    }

    /// See [`layouter::Layouter::take_recorded_glyphs`].
    pub fn take_recorded_glyphs(&mut self) -> Vec<layouter::RecordedFont> {
        self.layouter.take_recorded_glyphs()
    }

    /// See [`layouter::Layouter::missing_glyphs`].
    pub fn missing_glyphs(&self) -> usize {
        self.layouter.missing_glyphs()
    }

    /// The end of a draw: the glyphs it asked for go to the host.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn prepare_textures(&mut self, cx: &mut Cx) -> bool {
        // Frame boundary for the layout cache's working-set protection.
        self.layouter.advance_cache_generation();
        let outbox = {
            let mut rasterizer = self.layouter.rasterizer().borrow_mut();
            let client = rasterizer.client_mut();
            if client.sweep_due() {
                // what the live draw lists show is in use, redrawn or not
                for list in cx.draw_lists.iter_live() {
                    for glyph in &list.glyph_uses {
                        client.retained(*glyph);
                    }
                }
            }
            client.end_frame();
            client.take_outbox()
        };
        // fonts first: a request for a font names it by the id it was opened under
        for open in &outbox.opens {
            cx.hal_font_open(open.local, open.index, &open.vars, open.source());
        }
        for (local, kind, glyphs) in &outbox.requests {
            cx.hal_glyph_request(*local, *kind, glyphs);
        }
        cx.hal_glyphs_pending(&outbox.pending);
        if !outbox.releases.is_empty() {
            cx.hal_glyph_release(&outbox.releases);
        }
        if !outbox.touches.is_empty() {
            cx.hal_glyph_touch(&outbox.touches);
        }
        self.layouter.rasterizer().borrow_mut().client_mut().recycle(outbox);
        true
    }

    #[cfg(target_arch = "wasm32")]
    pub fn prepare_textures(&mut self, cx: &mut Cx) -> bool {
        assert!(!self.needs_prepare_atlases);
        // Frame boundary for the layout cache's working-set protection.
        self.layouter.advance_cache_generation();
        let mut rasterizer = self.layouter.rasterizer().borrow_mut();
        if rasterizer.color_atlas_mut().reset_if_needed() {
            rasterizer.on_atlas_reset();
            return false;
        }
        drop(rasterizer);
        // Same idea for the append-only slug glyph atlas: if it grew past its cap, clear it
        // here (before any upload below) and force a full redraw so every slug glyph rebuilds
        // into the fresh atlas. Returning false leaves the existing slug textures intact for
        // the current frame's GPU render.
        if self.slug_atlas.reset_if_needed() {
            return false;
        }
        let completed = self.apply_completed_msdf_jobs();
        if completed > 0 {
            cx.redraw_all();
        }
        self.dispatch_msdf_jobs();
        let slug_changed = self.flush_slug_textures(cx);
        if slug_changed {
            cx.redraw_all();
        }
        self.prepare_atlas_texture(cx);
        self.needs_prepare_atlases = true;
        true
    }

    #[cfg(target_arch = "wasm32")]
    fn prepare_atlas_texture(&mut self, cx: &mut Cx) {
        let mut rasterizer = self.layouter.rasterizer().borrow_mut();
        let dirty_rect = rasterizer.color_atlas_mut().take_dirty_image().bounds();
        let pixels = bgra_vec_into_u32(rasterizer.color_atlas_mut().take_pixels());
        self.atlas_texture.put_back_vec_u32(
            cx,
            pixels,
            Some(RectUsize::new(
                PointUsize::new(dirty_rect.origin.x, dirty_rect.origin.y),
                SizeUsize::new(dirty_rect.size.width, dirty_rect.size.height),
            )),
        )
    }

    /// The start of a draw. Under the HAL: takes what the font service answered since the last one
    /// (the atlas textures lent to this session, the glyphs that arrived or were taken back).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn prepare_atlases_if_needed(&mut self, cx: &mut Cx) {
        use crate::makepad_platform::{hal_font_wire as wire, HalFontEvent};
        for event in cx.hal_take_font_events() {
            let mut rasterizer = self.layouter.rasterizer().borrow_mut();
            let client = rasterizer.client_mut();
            match event {
                HalFontEvent::Opened { font, status, len: _, hash: _, path } => {
                    if status != wire::font_status::OPENED {
                        crate::error!("the font service could not open a font ({}): {path}", if status == wire::font_status::MISSING { "missing" } else { "not a font" });
                    }
                    client.on_opened(font, status);
                }
                HalFontEvent::Atlas { kind, texture, width, height, generation } => {
                    client.on_atlas(kind, width, height, generation);
                    if kind == wire::atlas::RASTER {
                        self.atlas_texture.set_hal_lent(cx, texture);
                    } else {
                        self.slug_curve_texture.set_hal_lent(cx, texture);
                    }
                }
                HalFontEvent::Ready { font, entries } => client.on_ready(font, &entries),
                HalFontEvent::Evicted { generation: _, slots, .. } => client.on_evicted(&slots),
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn prepare_atlases_if_needed(&mut self, cx: &mut Cx) {
        if !self.needs_prepare_atlases {
            return;
        }
        self.prepare_atlas(cx);
        self.needs_prepare_atlases = false;
    }

    #[cfg(target_arch = "wasm32")]
    fn prepare_atlas(&mut self, cx: &mut Cx) {
        let mut rasterizer = self.layouter.rasterizer().borrow_mut();
        let pixels = self.atlas_texture.take_vec_u32(cx);
        let pixels = u32_vec_into_bgra(pixels);
        rasterizer.color_atlas_mut().replace_pixels(pixels);
    }

    #[cfg(target_arch = "wasm32")]
    fn dispatch_msdf_jobs(&mut self) {
        let jobs = self
            .layouter
            .rasterizer()
            .borrow_mut()
            .take_queued_msdf_jobs();
        for job in jobs {
            if self.msdf_job_sender.send(job).is_ok() {
                self.msdf_jobs_in_flight += 1;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn apply_completed_msdf_jobs(&mut self) -> usize {
        let mut completed = 0usize;
        while let Ok(job) = self.msdf_result_receiver.try_recv() {
            self.layouter
                .rasterizer()
                .borrow_mut()
                .apply_completed_msdf_job(job);
            completed += 1;
            self.msdf_jobs_in_flight = self.msdf_jobs_in_flight.saturating_sub(1);
        }
        self.msdf_applied += completed;
        completed
    }

    /// Glyph upgrades finished on the MSDF worker and not applied yet (they are applied at
    /// the next draw, which the signal hook requests).
    pub fn msdf_results_waiting(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        return false;
        #[cfg(target_arch = "wasm32")]
        (self.msdf_sent.load(std::sync::atomic::Ordering::Acquire) > self.msdf_applied)
    }
}

#[cfg(target_arch = "wasm32")]
fn bgra_vec_into_u32(vec: Vec<Bgra>) -> Vec<u32> {
    debug_assert_eq!(std::mem::size_of::<Bgra>(), std::mem::size_of::<u32>());
    debug_assert_eq!(std::mem::align_of::<Bgra>(), std::mem::align_of::<u32>());
    let mut vec = ManuallyDrop::new(vec);
    // SAFETY:
    // `Bgra` is `#[repr(transparent)]` over `u32`, so element layout matches exactly.
    // We preserve the same pointer/len/cap and only reinterpret the element type.
    unsafe { Vec::from_raw_parts(vec.as_mut_ptr().cast::<u32>(), vec.len(), vec.capacity()) }
}

#[cfg(target_arch = "wasm32")]
fn u32_vec_into_bgra(vec: Vec<u32>) -> Vec<Bgra> {
    debug_assert_eq!(std::mem::size_of::<Bgra>(), std::mem::size_of::<u32>());
    debug_assert_eq!(std::mem::align_of::<Bgra>(), std::mem::align_of::<u32>());
    let mut vec = ManuallyDrop::new(vec);
    // SAFETY:
    // `Bgra` is `#[repr(transparent)]` over `u32`, so element layout matches exactly.
    // We preserve the same pointer/len/cap and only reinterpret the element type.
    unsafe { Vec::from_raw_parts(vec.as_mut_ptr().cast::<Bgra>(), vec.len(), vec.capacity()) }
}

#[cfg(test)]
mod tests {
    use super::LazyFontRequests;
    use crate::{
        makepad_platform::LazyFontFamily,
        text::font_family::FontFamilyId,
    };

    #[test]
    fn glyph_miss_requests_each_lazy_family_once() {
        let family_id: FontFamilyId = 0xFA17_u64.into();
        let mut requests = LazyFontRequests::default();
        assert!(!requests.take_for_glyph_miss(
            family_id,
            LazyFontFamily::Cjk,
            "\u{4f60}",
            false,
        ));
        assert!(requests.take_for_glyph_miss(
            family_id,
            LazyFontFamily::Cjk,
            "\u{4f60}",
            true,
        ));
        assert!(!requests.take_for_glyph_miss(
            family_id,
            LazyFontFamily::Cjk,
            "\u{597d}",
            true,
        ));
        assert!(requests.take_for_glyph_miss(
            family_id,
            LazyFontFamily::Emoji,
            "\u{1f600}",
            true,
        ));
        assert!(!requests.take_for_glyph_miss(
            family_id,
            LazyFontFamily::Emoji,
            "\u{1f680}",
            true,
        ));
    }
}
