use {
    super::{
        font::{Font, FontId, GlyphId},
        font_family::{FontFamily, FontFamilyId},
        image::{Bgra, Image},
        layouter::{self, LaidoutText, LayoutParams, Layouter},
        loader::{FontDefinition, FontFamilyDefinition},
        msdfer::Msdfer,
        rasterizer::{CompletedMsdfJob, OutlineRasterizationMode, QueuedMsdfJob, Rasterizer},
        slug_atlas::{SlugAtlas, SlugGlyphCacheResult},
    },
    crate::makepad_platform::*,
    std::{cell::RefCell, mem::ManuallyDrop, rc::Rc},
};

fn default_slug_new_glyphs_per_redraw(cx: &Cx) -> usize {
    match cx.os_type() {
        OsType::LinuxWindow(_) | OsType::LinuxDirect | OsType::Windows => 1,
        _ => usize::MAX,
    }
}

fn default_slug_min_dpxs_per_em(cx: &Cx, rasterizer: &Rasterizer) -> f32 {
    match cx.os_type() {
        OsType::LinuxWindow(_) | OsType::LinuxDirect | OsType::Windows => {
            rasterizer.msdf_resolution().max_dpxs_per_em
        }
        _ => 0.0,
    }
}

pub struct Fonts {
    layouter: Layouter,
    needs_prepare_atlases: bool,
    atlas_texture: Texture,
    slug_atlas: SlugAtlas,
    slug_min_dpxs_per_em: f32,
    slug_new_glyphs_per_redraw: usize,
    slug_budget_redraw_id: u64,
    slug_built_glyphs_this_redraw: usize,
    msdf_job_sender: FromUISender<QueuedMsdfJob>,
    msdf_result_receiver: ToUIReceiver<CompletedMsdfJob>,
}

impl Fonts {
    pub fn new(cx: &mut Cx, settings: layouter::Settings) -> Self {
        let layouter = Layouter::new(settings);
        let (atlas_size, msdfer_settings, slug_min_dpxs_per_em) = {
            let rasterizer = layouter.rasterizer().borrow();
            (
                rasterizer.color_atlas().size(),
                rasterizer.msdfer().settings(),
                default_slug_min_dpxs_per_em(cx, &rasterizer),
            )
        };

        let mut msdf_job_sender: FromUISender<QueuedMsdfJob> = Default::default();
        let msdf_result_receiver: ToUIReceiver<CompletedMsdfJob> = Default::default();
        let worker_rx = msdf_job_sender.receiver();
        let worker_tx = msdf_result_receiver.sender();
        cx.spawn_thread(move || {
            let mut msdfer = Msdfer::new(msdfer_settings);
            while let Ok(job) = worker_rx.recv() {
                let mut msdf = Image::<Bgra>::new(job.key.size);
                msdfer.outline_to_msdf(
                    &job.outline,
                    job.dpxs_per_em,
                    &mut msdf.subimage_mut(super::geom::Rect::from(job.key.size)),
                );
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
        });

        Self {
            layouter,
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
            slug_min_dpxs_per_em,
            slug_new_glyphs_per_redraw: default_slug_new_glyphs_per_redraw(cx),
            slug_budget_redraw_id: 0,
            slug_built_glyphs_this_redraw: 0,
            msdf_job_sender,
            msdf_result_receiver,
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
        self.slug_atlas.curve_texture()
    }

    pub fn slug_band_texture(&self) -> &Texture {
        self.slug_atlas.band_texture()
    }

    pub fn should_use_slug_glyph(&self, dpxs_per_em: f32) -> bool {
        dpxs_per_em >= self.slug_min_dpxs_per_em
    }

    pub fn max_rasterized_glyph_dpxs_per_em(&self) -> f32 {
        self.layouter
            .rasterizer()
            .borrow()
            .msdf_resolution()
            .max_dpxs_per_em
    }

    pub fn get_or_cache_slug_glyph(
        &mut self,
        redraw_id: u64,
        font: &Font,
        glyph_id: GlyphId,
    ) -> SlugGlyphCacheResult {
        match self.slug_atlas.get_or_cache_glyph(font, glyph_id, false) {
            SlugGlyphCacheResult::Deferred => {}
            result => return result,
        }

        if self.slug_new_glyphs_per_redraw != usize::MAX {
            if self.slug_budget_redraw_id != redraw_id {
                self.slug_budget_redraw_id = redraw_id;
                self.slug_built_glyphs_this_redraw = 0;
            }
            if self.slug_built_glyphs_this_redraw >= self.slug_new_glyphs_per_redraw {
                return SlugGlyphCacheResult::Deferred;
            }
            self.slug_built_glyphs_this_redraw += 1;
        }

        self.slug_atlas.get_or_cache_glyph(font, glyph_id, true)
    }

    pub fn slug_cache_generation(&self) -> u64 {
        self.slug_atlas.cache_generation()
    }

    pub fn slug_uploaded_generation(&self) -> u64 {
        self.slug_atlas.uploaded_generation()
    }

    /// Uploads any newly appended SLUG curve/band data immediately so draw calls
    /// in the current frame can see glyphs cached during the draw loop.
    pub fn flush_slug_textures(&mut self, cx: &mut Cx) -> bool {
        self.slug_atlas.prepare_textures(cx)
    }

    pub fn is_font_family_known(&self, id: FontFamilyId) -> bool {
        self.layouter.is_font_family_known(id)
    }

    pub fn is_font_family_complete(&self, id: FontFamilyId) -> bool {
        self.layouter
            .loader
            .font_family_definitions
            .get(&id)
            .map(|def| def.font_ids.len() == def.expected_member_count)
            .unwrap_or(false)
    }

    pub fn is_font_known(&self, id: FontId) -> bool {
        self.layouter.is_font_known(id)
    }

    pub fn define_font_family(&mut self, id: FontFamilyId, definition: FontFamilyDefinition) {
        self.layouter.define_font_family(id, definition);
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

    pub fn prepare_textures(&mut self, cx: &mut Cx) -> bool {
        assert!(!self.needs_prepare_atlases);
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

    pub fn prepare_atlases_if_needed(&mut self, cx: &mut Cx) {
        if !self.needs_prepare_atlases {
            return;
        }
        self.prepare_atlas(cx);
        self.needs_prepare_atlases = false;
    }

    fn prepare_atlas(&mut self, cx: &mut Cx) {
        let mut rasterizer = self.layouter.rasterizer().borrow_mut();
        let pixels = self.atlas_texture.take_vec_u32(cx);
        let pixels = u32_vec_into_bgra(pixels);
        rasterizer.color_atlas_mut().replace_pixels(pixels);
    }

    fn dispatch_msdf_jobs(&mut self) {
        let jobs = self
            .layouter
            .rasterizer()
            .borrow_mut()
            .take_queued_msdf_jobs();
        for job in jobs {
            let _ = self.msdf_job_sender.send(job);
        }
    }

    fn apply_completed_msdf_jobs(&mut self) -> usize {
        let mut completed = 0usize;
        while let Ok(job) = self.msdf_result_receiver.try_recv() {
            self.layouter
                .rasterizer()
                .borrow_mut()
                .apply_completed_msdf_job(job);
            completed += 1;
        }
        completed
    }
}

fn bgra_vec_into_u32(vec: Vec<Bgra>) -> Vec<u32> {
    debug_assert_eq!(std::mem::size_of::<Bgra>(), std::mem::size_of::<u32>());
    debug_assert_eq!(std::mem::align_of::<Bgra>(), std::mem::align_of::<u32>());
    let mut vec = ManuallyDrop::new(vec);
    // SAFETY:
    // `Bgra` is `#[repr(transparent)]` over `u32`, so element layout matches exactly.
    // We preserve the same pointer/len/cap and only reinterpret the element type.
    unsafe { Vec::from_raw_parts(vec.as_mut_ptr().cast::<u32>(), vec.len(), vec.capacity()) }
}

fn u32_vec_into_bgra(vec: Vec<u32>) -> Vec<Bgra> {
    debug_assert_eq!(std::mem::size_of::<Bgra>(), std::mem::size_of::<u32>());
    debug_assert_eq!(std::mem::align_of::<Bgra>(), std::mem::align_of::<u32>());
    let mut vec = ManuallyDrop::new(vec);
    // SAFETY:
    // `Bgra` is `#[repr(transparent)]` over `u32`, so element layout matches exactly.
    // We preserve the same pointer/len/cap and only reinterpret the element type.
    unsafe { Vec::from_raw_parts(vec.as_mut_ptr().cast::<Bgra>(), vec.len(), vec.capacity()) }
}
