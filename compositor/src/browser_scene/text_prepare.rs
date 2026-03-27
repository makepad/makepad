use std::collections::HashMap;

use makepad_draw::text::font::{Font, GlyphId};
use makepad_draw::text::geom::{Point, Rect as TextRect, Size};
use makepad_draw::text::glyph_outline::{Command, GlyphOutline};
use makepad_draw::text::image::{Bgra, Image, R};
use makepad_draw::text::msdfer::{self, Msdfer};
use makepad_draw::text::rasterizer::{MsdfComplexitySettings, OutlineRasterizationMode};
use makepad_draw::text::sdfer::{self, Sdfer};

use std::cell::RefCell;
use std::rc::Rc;

use super::fonts::resolve_font_with_fonts;
use super::{MpBrowserScene, MpBrowserTaskKind, MpBrowserTextRun};
use crate::*;

#[path = "text_prepare_atlas.rs"]
mod atlas;
use atlas::*;

#[cfg(test)]
#[path = "text_prepare_tests.rs"]
mod tests;
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct BrowserFontFaceKey {
    pub resource_key: u64,
    pub face_index: u32,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct BrowserFontInstanceKey {
    pub face_key: BrowserFontFaceKey,
    pub format: BrowserGlyphFormat,
    pub quantized_dpx_per_em: u16,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct BrowserGlyphKey {
    pub instance_key: BrowserFontInstanceKey,
    pub glyph_id: GlyphId,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct BrowserGlyphAtlasPageId(pub u32);
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum BrowserGlyphFormat {
    Alpha,
    Color,
    Msdf,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct BrowserAtlasPageRef {
    pub format: BrowserGlyphFormat,
    pub page_id: BrowserGlyphAtlasPageId,
    pub page_generation: u64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct BrowserGlyphEntry {
    pub generation: u64,
    pub page: BrowserAtlasPageRef,
    pub atlas_page_size: Size<usize>,
    pub atlas_image_bounds: TextRect<usize>,
    pub atlas_image_padding: usize,
    pub atlas_plane: u8,
    pub origin_in_dpxs: Point<f32>,
    pub dpxs_per_em: f32,
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MpPreparedBrowserTextRunKey {
    pub retained_scene_id: crate::browser_scene::MpBrowserRetainedSceneId,
    pub stable_text_run_id: crate::browser_scene::MpBrowserStableTextRunId,
}
#[derive(Clone, Debug, Default)]
pub struct MpPreparedBrowserScene {
    retained_scene_id: crate::browser_scene::MpBrowserRetainedSceneId,
    text_runs: HashMap<crate::browser_scene::MpBrowserStableTextRunId, PreparedBrowserTextRun>,
}
#[derive(Clone, Debug)]
pub struct PreparedBrowserTextRun {
    pub(super) key: MpPreparedBrowserTextRunKey,
    pub(super) reset_epoch: u64,
    pub(super) batches: Vec<PreparedBrowserGlyphBatch>,
}
impl MpPreparedBrowserScene {
    pub fn new(retained_scene_id: crate::browser_scene::MpBrowserRetainedSceneId) -> Self {
        Self {
            retained_scene_id,
            text_runs: HashMap::new(),
        }
    }

    pub fn retained_scene_id(&self) -> crate::browser_scene::MpBrowserRetainedSceneId {
        self.retained_scene_id
    }

    pub(super) fn prepared_run(
        &self,
        stable_id: crate::browser_scene::MpBrowserStableTextRunId,
    ) -> Option<&PreparedBrowserTextRun> {
        self.text_runs.get(&stable_id)
    }
}
#[derive(Clone, Debug)]
pub(super) struct PreparedBrowserGlyphBatch {
    pub(super) page: BrowserAtlasPageRef,
    pub(super) glyphs: Vec<PreparedBrowserGlyph>,
}
#[derive(Clone, Debug)]
pub(super) struct PreparedBrowserGlyph {
    pub(super) origin: DVec2,
    pub(super) font_size_px: f32,
    pub(super) glyph_key: BrowserGlyphKey,
    pub(super) entry: BrowserGlyphEntry,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct BrowserPageBinding<'a> {
    pub format: BrowserGlyphFormat,
    pub generation: u64,
    pub texture: &'a Texture,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct BrowserTextPrepareStats {
    pub prepared_text_batch_hit_count: usize,
    pub prepared_text_batch_miss_count: usize,
    pub prepared_text_batch_rebuild_count: usize,
    pub glyph_residency_hit_count: usize,
    pub glyph_residency_miss_count: usize,
    pub glyph_cache_reset_count: usize,
    pub atlas_page_alloc_count: usize,
    pub msdf_request_queue_count: usize,
    pub msdf_completion_count: usize,
    pub synchronous_fallback_glyph_generation_count: usize,
}
#[derive(Clone, Debug)]
struct BrowserQueuedMsdfJob {
    key: BrowserGlyphKey,
    outline: GlyphOutline,
    dpxs_per_em: f32,
    image_size: Size<usize>,
    epoch: u64,
}
#[derive(Clone, Debug)]
struct BrowserCompletedMsdfJob {
    key: BrowserGlyphKey,
    pixels: Vec<Bgra>,
    image_size: Size<usize>,
    epoch: u64,
}

pub(super) struct BrowserTextCache {
    reset_epoch: u64,
    next_page_id: u32,
    next_page_generation: u64,
    next_glyph_generation: u64,
    next_msdf_job_epoch: u64,
    settings: BrowserTextRasterSettings,
    sdfer: Sdfer,
    msdfer: Msdfer,
    glyphs: HashMap<BrowserGlyphKey, BrowserGlyphEntry>,
    pending_msdf: HashMap<BrowserGlyphKey, u64>,
    queued_msdf_jobs: Vec<BrowserQueuedMsdfJob>,
    msdf_job_sender: Option<FromUISender<BrowserQueuedMsdfJob>>,
    msdf_result_receiver: Option<ToUIReceiver<BrowserCompletedMsdfJob>>,
    msdf_worker_settings: Option<BrowserTextRasterSettings>,
    alpha_pages: Vec<BrowserGlyphAtlasPage>,
    color_pages: Vec<BrowserGlyphAtlasPage>,
    msdf_pages: Vec<BrowserGlyphAtlasPage>,
    frame_stats: BrowserTextPrepareStats,
}
impl BrowserTextCache {
    pub(super) fn new() -> Self {
        let settings = BrowserTextRasterSettings::default();
        Self {
            reset_epoch: 1,
            next_page_id: 1,
            next_page_generation: 1,
            next_glyph_generation: 1,
            next_msdf_job_epoch: 1,
            settings,
            sdfer: Sdfer::new(settings.sdfer_settings()),
            msdfer: Msdfer::new(settings.msdf_settings()),
            glyphs: HashMap::new(),
            pending_msdf: HashMap::new(),
            queued_msdf_jobs: Vec::new(),
            msdf_job_sender: None,
            msdf_result_receiver: None,
            msdf_worker_settings: None,
            alpha_pages: Vec::new(),
            color_pages: Vec::new(),
            msdf_pages: Vec::new(),
            frame_stats: BrowserTextPrepareStats::default(),
        }
    }

    pub(super) fn frame_stats(&self) -> BrowserTextPrepareStats {
        self.frame_stats
    }

    pub(super) fn prepare_retained_scene(
        &mut self,
        cx: &mut Cx2d,
        scene: &MpBrowserScene,
        prepared: &mut MpPreparedBrowserScene,
    ) {
        let dpi_factor = cx.current_dpi_factor();
        let fonts_rc = cx
            .cx
            .get_global::<Rc<RefCell<makepad_draw::text::fonts::Fonts>>>()
            .clone();
        self.prepare_retained_scene_with_fonts(cx.cx, scene, prepared, dpi_factor, &fonts_rc);
    }

    #[cfg(test)]
    pub(super) fn prepare_retained_scene_for_test(
        &mut self,
        cx: &mut Cx,
        scene: &MpBrowserScene,
        prepared: &mut MpPreparedBrowserScene,
        dpi_factor: f64,
        fonts_rc: &Rc<RefCell<makepad_draw::text::fonts::Fonts>>,
    ) {
        self.prepare_retained_scene_with_fonts(cx, scene, prepared, dpi_factor, fonts_rc);
    }

    pub(super) fn page_binding(&self, page: BrowserAtlasPageRef) -> Option<BrowserPageBinding<'_>> {
        let atlas_page = self.find_page(page)?;
        Some(BrowserPageBinding {
            format: page.format,
            generation: atlas_page.generation,
            texture: &atlas_page.texture,
        })
    }

    pub(super) fn is_prepared_run_valid(&self, run: &PreparedBrowserTextRun) -> bool {
        if run.reset_epoch != self.reset_epoch {
            return false;
        }
        run.batches.iter().all(|batch| {
            self.page_binding(batch.page)
                .map(|binding| binding.generation == batch.page.page_generation)
                .unwrap_or(false)
                && batch.glyphs.iter().all(|glyph| {
                    self.glyphs
                        .get(&glyph.glyph_key)
                        .map(|entry| entry.generation == glyph.entry.generation)
                        .unwrap_or(false)
                })
        })
    }

    pub(super) fn reset(&mut self) {
        self.glyphs.clear();
        self.pending_msdf.clear();
        self.queued_msdf_jobs.clear();
        self.alpha_pages.clear();
        self.color_pages.clear();
        self.msdf_pages.clear();
        self.next_page_id = 1;
        self.next_page_generation = 1;
        self.reset_epoch = self.reset_epoch.wrapping_add(1);
        self.frame_stats.glyph_cache_reset_count += 1;
    }

    fn prepare_retained_scene_with_fonts(
        &mut self,
        cx: &mut Cx,
        scene: &MpBrowserScene,
        prepared: &mut MpPreparedBrowserScene,
        dpi_factor: f64,
        fonts_rc: &Rc<RefCell<makepad_draw::text::fonts::Fonts>>,
    ) {
        self.frame_stats = BrowserTextPrepareStats::default();
        self.sync_settings_from_fonts_with_fonts(fonts_rc);
        self.ensure_msdf_worker(cx);
        let completions_before_prepare = self.apply_completed_msdf_jobs(cx);
        if prepared.retained_scene_id != scene.retained_scene_id {
            prepared.text_runs.clear();
        }
        prepared.retained_scene_id = scene.retained_scene_id;
        self.prepare_scene_inner(cx, scene, prepared, dpi_factor, fonts_rc);
        self.dispatch_msdf_jobs();
        let completions_before_upload = self.apply_completed_msdf_jobs(cx);
        if completions_before_prepare + completions_before_upload > 0 {
            cx.redraw_all();
        }
        self.upload_dirty_pages(cx);
    }

    fn prepare_scene_inner(
        &mut self,
        cx: &mut Cx,
        scene: &MpBrowserScene,
        prepared: &mut MpPreparedBrowserScene,
        dpi_factor: f64,
        fonts_rc: &Rc<RefCell<makepad_draw::text::fonts::Fonts>>,
    ) {
        prepared
            .text_runs
            .retain(|stable_id, _| scene_contains_text_run(scene, *stable_id));
        for text_run in &scene.text_runs {
            let key = MpPreparedBrowserTextRunKey {
                retained_scene_id: scene.retained_scene_id,
                stable_text_run_id: text_run.stable_id,
            };
            let valid = prepared
                .text_runs
                .get(&text_run.stable_id)
                .filter(|run| run.key == key)
                .map(|run| self.is_prepared_run_valid(run))
                .unwrap_or(false);
            if valid {
                self.frame_stats.prepared_text_batch_hit_count += 1;
                continue;
            }
            let rebuilt = self.prepare_text_run(cx, text_run, key, dpi_factor, fonts_rc);
            if let Some(run) = rebuilt {
                prepared.text_runs.insert(text_run.stable_id, run);
            } else {
                prepared.text_runs.remove(&text_run.stable_id);
            }
        }
        for task in &scene.tasks {
            if let MpBrowserTaskKind::Scene(task_scene) = &task.kind {
                self.prepare_scene_inner(cx, task_scene, prepared, dpi_factor, fonts_rc);
            }
        }
    }

    fn prepare_text_run(
        &mut self,
        cx: &mut Cx,
        text_run: &MpBrowserTextRun,
        key: MpPreparedBrowserTextRunKey,
        dpi_factor: f64,
        fonts_rc: &Rc<RefCell<makepad_draw::text::fonts::Fonts>>,
    ) -> Option<PreparedBrowserTextRun> {
        self.frame_stats.prepared_text_batch_miss_count += 1;
        let mut batches: Vec<PreparedBrowserGlyphBatch> = Vec::new();
        for glyph in &text_run.glyphs {
            let font_resource = text_run.fonts.get(glyph.font_slot as usize)?;
            let font = resolve_font_with_fonts(fonts_rc, font_resource)?;
            let (glyph_key, entry) = self.resolve_glyph(
                cx,
                BrowserFontFaceKey {
                    resource_key: font_resource.key,
                    face_index: font_resource.face_index,
                },
                font.as_ref(),
                glyph.glyph_id as GlyphId,
                glyph.font_size_px * dpi_factor as f32,
            )?;
            let prepared_glyph = PreparedBrowserGlyph {
                origin: glyph.origin,
                font_size_px: glyph.font_size_px,
                glyph_key,
                entry,
            };
            match batches.last_mut() {
                Some(batch) if batch.page == entry.page => batch.glyphs.push(prepared_glyph),
                _ => batches.push(PreparedBrowserGlyphBatch {
                    page: entry.page,
                    glyphs: vec![prepared_glyph],
                }),
            }
        }
        if batches.is_empty() {
            return None;
        }
        self.frame_stats.prepared_text_batch_rebuild_count += 1;
        Some(PreparedBrowserTextRun {
            key,
            reset_epoch: self.reset_epoch,
            batches,
        })
    }

    fn resolve_glyph(
        &mut self,
        cx: &mut Cx,
        face_key: BrowserFontFaceKey,
        font: &Font,
        glyph_id: GlyphId,
        requested_dpxs_per_em: f32,
    ) -> Option<(BrowserGlyphKey, BrowserGlyphEntry)> {
        let request = BrowserGlyphRequest::new(face_key, glyph_id, requested_dpxs_per_em, self.settings);
        if let Some(entry) = self.glyphs.get(&request.key).copied() {
            self.frame_stats.glyph_residency_hit_count += 1;
            return Some((request.key, entry));
        }
        self.frame_stats.glyph_residency_miss_count += 1;
        if font.glyph_outline_bounds_in_ems(glyph_id, &mut None).is_some() {
            self.resolve_outline_glyph(cx, font, request)
                .map(|entry| (request.key, entry))
        } else {
            self.resolve_color_glyph(cx, font, request)
                .map(|entry| (request.key, entry))
        }
    }

    fn resolve_outline_glyph(
        &mut self,
        cx: &mut Cx,
        font: &Font,
        request: BrowserGlyphRequest,
    ) -> Option<BrowserGlyphEntry> {
        let mut outline = None;
        let bounds_in_ems = font.glyph_outline_bounds_in_ems(request.key.glyph_id, &mut outline)?;
        let outline = outline.or_else(|| font.glyph_outline(request.key.glyph_id))?;
        let request = request.with_outline_policy(&outline);
        if let Some(entry) = self.glyphs.get(&request.key).copied() {
            return Some(entry);
        }
        let image_size = glyph_outline_image_size(bounds_in_ems.size, request.dpxs_per_em)
            + Size::from(request.padding) * 2;
        let origin_in_dpxs = bounds_in_ems.origin * request.dpxs_per_em;
        match request.key.instance_key.format {
            BrowserGlyphFormat::Alpha => {
                self.publish_outline_fallback(
                    cx,
                    request.key,
                    &outline,
                    image_size,
                    origin_in_dpxs,
                    request.dpxs_per_em,
                    request.padding,
                )
            }
            BrowserGlyphFormat::Msdf => {
                let entry = self.publish_outline_fallback(
                    cx,
                    request.key,
                    &outline,
                    image_size,
                    origin_in_dpxs,
                    request.dpxs_per_em,
                    request.padding,
                )?;
                self.queue_msdf_promotion(request.key, outline, request.dpxs_per_em, image_size);
                Some(entry)
            }
            BrowserGlyphFormat::Color => None,
        }
    }

    fn resolve_color_glyph(
        &mut self,
        cx: &mut Cx,
        font: &Font,
        request: BrowserGlyphRequest,
    ) -> Option<BrowserGlyphEntry> {
        const COLOR_PADDING: usize = 2;
        let request = request.force_color(COLOR_PADDING);
        if let Some(entry) = self.glyphs.get(&request.key).copied() {
            return Some(entry);
        }
        font.with_glyph_raster_image(request.key.glyph_id, request.dpxs_per_em, |raster_image| {
            self.frame_stats.synchronous_fallback_glyph_generation_count += 1;
            let image_size = raster_image.decode_size() + Size::from(COLOR_PADDING * 2);
            let (page_ref, rect) = self.allocate_page_rect(cx, BrowserGlyphFormat::Color, image_size)?;
            let mut image = Image::<Bgra>::new(image_size);
            raster_image.decode(&mut image.subimage_mut(TextRect::new(
                Point::new(COLOR_PADDING, COLOR_PADDING),
                raster_image.decode_size(),
            )));
            let generation = next_generation(&mut self.next_glyph_generation);
            let page = self.page_mut(page_ref.page_id);
            blit_bgra_image(page, rect, &image);
            let entry = BrowserGlyphEntry {
                generation,
                page: page.page_ref(),
                atlas_page_size: page.size,
                atlas_image_bounds: rect,
                atlas_image_padding: COLOR_PADDING,
                atlas_plane: 0,
                origin_in_dpxs: raster_image.origin_in_dpxs(),
                dpxs_per_em: raster_image.dpxs_per_em(),
            };
            self.glyphs.insert(request.key, entry);
            Some(entry)
        })?
    }

    fn publish_outline_fallback(
        &mut self,
        cx: &mut Cx,
        glyph_key: BrowserGlyphKey,
        outline: &GlyphOutline,
        image_size: Size<usize>,
        origin_in_dpxs: Point<f32>,
        dpxs_per_em: f32,
        padding: usize,
    ) -> Option<BrowserGlyphEntry> {
        self.frame_stats.synchronous_fallback_glyph_generation_count += 1;
        let coverage_size = Size::new(
            image_size.width.saturating_sub(padding * 2),
            image_size.height.saturating_sub(padding * 2),
        );
        let mut coverage = Image::<R>::new(coverage_size);
        outline.rasterize(
            dpxs_per_em,
            &mut coverage.subimage_mut(TextRect::from(coverage_size)),
        );
        let mut sdf = Image::<R>::new(image_size);
        self.sdfer.coverage_to_sdf(
            &coverage.subimage(TextRect::from(coverage_size)),
            &mut sdf.subimage_mut(TextRect::from(image_size)),
        );
        let (page_ref, rect) = self.allocate_page_rect(cx, BrowserGlyphFormat::Alpha, image_size)?;
        let generation = next_generation(&mut self.next_glyph_generation);
        let page = self.page_mut(page_ref.page_id);
        blit_alpha_image(page, rect, &sdf);
        let entry = BrowserGlyphEntry {
            generation,
            page: page.page_ref(),
            atlas_page_size: page.size,
            atlas_image_bounds: rect,
            atlas_image_padding: padding,
            atlas_plane: 3,
            origin_in_dpxs,
            dpxs_per_em,
        };
        self.glyphs.insert(glyph_key, entry);
        Some(entry)
    }

    fn queue_msdf_promotion(
        &mut self,
        key: BrowserGlyphKey,
        outline: GlyphOutline,
        dpxs_per_em: f32,
        image_size: Size<usize>,
    ) {
        if self.pending_msdf.contains_key(&key) {
            return;
        }
        let epoch = next_generation(&mut self.next_msdf_job_epoch);
        self.pending_msdf.insert(key, epoch);
        self.queued_msdf_jobs.push(BrowserQueuedMsdfJob {
            key,
            outline,
            dpxs_per_em,
            image_size,
            epoch,
        });
        self.frame_stats.msdf_request_queue_count += 1;
    }

    fn apply_completed_msdf_job(&mut self, cx: &mut Cx, job: BrowserCompletedMsdfJob) -> bool {
        if self.pending_msdf.remove(&job.key) != Some(job.epoch) {
            return false;
        }
        let Some(fallback_entry) = self.glyphs.get(&job.key).copied() else {
            return false;
        };
        let Some((page_ref, rect)) = self.allocate_page_rect(cx, BrowserGlyphFormat::Msdf, job.image_size) else {
            return false;
        };
        let generation = next_generation(&mut self.next_glyph_generation);
        let page = self.page_mut(page_ref.page_id);
        blit_bgra_pixels(page, rect, &job.pixels, job.image_size);
        let promoted = BrowserGlyphEntry {
            generation,
            page: page.page_ref(),
            atlas_page_size: page.size,
            atlas_image_bounds: rect,
            atlas_image_padding: fallback_entry.atlas_image_padding,
            atlas_plane: 0,
            origin_in_dpxs: fallback_entry.origin_in_dpxs,
            dpxs_per_em: fallback_entry.dpxs_per_em,
        };
        self.glyphs.insert(job.key, promoted);
        true
    }

    fn allocate_page_rect(
        &mut self,
        cx: &mut Cx,
        format: BrowserGlyphFormat,
        size: Size<usize>,
    ) -> Option<(BrowserAtlasPageRef, TextRect<usize>)> {
        for page in self.pages_mut(format).iter_mut() {
            if let Some(rect) = page.allocate(size) {
                return Some((page.page_ref(), rect));
            }
        }
        let mut page = BrowserGlyphAtlasPage::new(
            cx,
            next_page_id(&mut self.next_page_id),
            next_generation(&mut self.next_page_generation),
            format,
            self.settings.page_size,
        );
        self.frame_stats.atlas_page_alloc_count += 1;
        let rect = page.allocate(size)?;
        let page_ref = page.page_ref();
        self.pages_mut(format).push(page);
        Some((page_ref, rect))
    }

    fn page_mut(&mut self, id: BrowserGlyphAtlasPageId) -> &mut BrowserGlyphAtlasPage {
        if let Some(page) = self.alpha_pages.iter_mut().find(|page| page.id == id) {
            return page;
        }
        if let Some(page) = self.color_pages.iter_mut().find(|page| page.id == id) {
            return page;
        }
        self.msdf_pages
            .iter_mut()
            .find(|page| page.id == id)
            .expect("browser text atlas page must exist")
    }

    fn find_page(&self, page_ref: BrowserAtlasPageRef) -> Option<&BrowserGlyphAtlasPage> {
        self.pages(page_ref.format)
            .iter()
            .find(|page| page.id == page_ref.page_id)
    }

    fn pages(&self, format: BrowserGlyphFormat) -> &[BrowserGlyphAtlasPage] {
        match format {
            BrowserGlyphFormat::Alpha => &self.alpha_pages,
            BrowserGlyphFormat::Color => &self.color_pages,
            BrowserGlyphFormat::Msdf => &self.msdf_pages,
        }
    }

    fn pages_mut(&mut self, format: BrowserGlyphFormat) -> &mut Vec<BrowserGlyphAtlasPage> {
        match format {
            BrowserGlyphFormat::Alpha => &mut self.alpha_pages,
            BrowserGlyphFormat::Color => &mut self.color_pages,
            BrowserGlyphFormat::Msdf => &mut self.msdf_pages,
        }
    }

    fn upload_dirty_pages(&mut self, cx: &mut Cx) {
        for page in self
            .alpha_pages
            .iter_mut()
            .chain(self.color_pages.iter_mut())
            .chain(self.msdf_pages.iter_mut())
        {
            page.upload_if_dirty(cx);
        }
    }

    fn ensure_msdf_worker(&mut self, cx: &mut Cx) {
        let worker_settings = self.settings;
        let msdf_settings = worker_settings.msdf_settings();
        if self.msdf_worker_settings == Some(worker_settings)
            && self.msdf_job_sender.is_some()
            && self.msdf_result_receiver.is_some()
        {
            return;
        }
        let mut msdf_job_sender: FromUISender<BrowserQueuedMsdfJob> = Default::default();
        let msdf_result_receiver: ToUIReceiver<BrowserCompletedMsdfJob> = Default::default();
        let worker_rx = msdf_job_sender.receiver();
        let worker_tx = msdf_result_receiver.sender();
        cx.spawn_thread(move || {
            let mut msdfer = Msdfer::new(msdf_settings);
            while let Ok(job) = worker_rx.recv() {
                let mut msdf = Image::<Bgra>::new(job.image_size);
                msdfer.outline_to_msdf(
                    &job.outline,
                    job.dpxs_per_em,
                    &mut msdf.subimage_mut(TextRect::from(job.image_size)),
                );
                if worker_tx
                    .send(BrowserCompletedMsdfJob {
                        key: job.key,
                        pixels: msdf.into_pixels(),
                        image_size: job.image_size,
                        epoch: job.epoch,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        self.msdf_job_sender = Some(msdf_job_sender);
        self.msdf_result_receiver = Some(msdf_result_receiver);
        self.msdf_worker_settings = Some(worker_settings);
    }

    fn dispatch_msdf_jobs(&mut self) {
        let Some(sender) = self.msdf_job_sender.as_ref() else {
            return;
        };
        for job in self.queued_msdf_jobs.drain(..) {
            let _ = sender.send(job);
        }
    }

    fn apply_completed_msdf_jobs(&mut self, cx: &mut Cx) -> usize {
        let mut completed = 0usize;
        loop {
            let recv = {
                let Some(receiver) = self.msdf_result_receiver.as_ref() else {
                    break;
                };
                receiver.try_recv()
            };
            match recv {
                Ok(job) => {
                    if self.apply_completed_msdf_job(cx, job) {
                        completed += 1;
                    }
                }
                Err(_) => break,
            }
        }
        if completed > 0 {
            self.frame_stats.msdf_completion_count += completed;
        }
        completed
    }

    fn sync_settings_from_fonts_with_fonts(
        &mut self,
        fonts_rc: &Rc<RefCell<makepad_draw::text::fonts::Fonts>>,
    ) {
        let settings = BrowserTextRasterSettings::from_fonts_handle(fonts_rc);
        if self.settings == settings {
            return;
        }
        self.settings = settings;
        self.sdfer = Sdfer::new(settings.sdfer_settings());
        self.msdfer = Msdfer::new(settings.msdf_settings());
        self.msdf_worker_settings = None;
        self.reset();
    }
}
#[derive(Clone, Copy, Debug)]
struct BrowserGlyphRequest {
    key: BrowserGlyphKey,
    requested_dpxs_per_em: f32,
    dpxs_per_em: f32,
    padding: usize,
    settings: BrowserTextRasterSettings,
}
impl BrowserGlyphRequest {
    fn new(
        face_key: BrowserFontFaceKey,
        glyph_id: GlyphId,
        requested_dpxs_per_em: f32,
        settings: BrowserTextRasterSettings,
    ) -> Self {
        let format = match settings.outline_rasterization_mode {
            OutlineRasterizationMode::Sdf => BrowserGlyphFormat::Alpha,
            OutlineRasterizationMode::Msdf => {
                if requested_dpxs_per_em <= settings.min_request_dpxs_per_em {
                    BrowserGlyphFormat::Alpha
                } else {
                    BrowserGlyphFormat::Msdf
                }
            }
        };
        let dpxs_per_em = quantize_outline_dpx(requested_dpxs_per_em, settings, format);
        let padding = match format {
            BrowserGlyphFormat::Alpha => settings.sdfer_padding,
            BrowserGlyphFormat::Msdf => settings.msdf_padding,
            BrowserGlyphFormat::Color => 2,
        };
        Self {
            key: BrowserGlyphKey {
                instance_key: BrowserFontInstanceKey {
                    face_key,
                    format,
                    quantized_dpx_per_em: dpxs_per_em as u16,
                },
                glyph_id,
            },
            requested_dpxs_per_em,
            dpxs_per_em,
            padding,
            settings,
        }
    }

    fn with_outline_policy(mut self, outline: &GlyphOutline) -> Self {
        if self.key.instance_key.format == BrowserGlyphFormat::Msdf {
            let complexity = estimate_outline_complexity(outline);
            if !is_msdf_complexity_acceptable(self.settings.msdf_complexity_settings(), complexity) {
                self.key.instance_key.format = BrowserGlyphFormat::Alpha;
                self.dpxs_per_em = quantize_outline_dpx(
                    self.dpxs_per_em,
                    self.settings,
                    BrowserGlyphFormat::Alpha,
                );
                self.key.instance_key.quantized_dpx_per_em = self.dpxs_per_em as u16;
                self.padding = self.settings.sdfer_padding;
            }
        }
        self
    }

    fn force_color(mut self, padding: usize) -> Self {
        self.key.instance_key.format = BrowserGlyphFormat::Color;
        self.dpxs_per_em = quantize_color_dpx(self.requested_dpxs_per_em);
        self.key.instance_key.quantized_dpx_per_em = self.dpxs_per_em as u16;
        self.padding = padding;
        self
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct BrowserTextRasterSettings {
    page_size: Size<usize>,
    sdfer_padding: usize,
    sdfer_radius: f32,
    sdfer_cutoff: f32,
    msdf_padding: usize,
    msdf_radius: f32,
    msdf_cutoff: f32,
    msdf_corner_angle_threshold: f32,
    min_request_dpxs_per_em: f32,
    min_dpxs_per_em: f32,
    max_dpxs_per_em: f32,
    dpx_quantum: f32,
    max_outline_commands: usize,
    max_estimated_segments: usize,
    outline_rasterization_mode: OutlineRasterizationMode,
}
impl Default for BrowserTextRasterSettings {
    fn default() -> Self {
        Self {
            page_size: Size::new(2048, 2048),
            sdfer_padding: 4,
            sdfer_radius: 8.0,
            sdfer_cutoff: 0.25,
            msdf_padding: 4,
            msdf_radius: 8.0,
            msdf_cutoff: 0.25,
            msdf_corner_angle_threshold: 3.0,
            min_request_dpxs_per_em: 20.0,
            min_dpxs_per_em: 32.0,
            max_dpxs_per_em: 128.0,
            dpx_quantum: 8.0,
            max_outline_commands: 180,
            max_estimated_segments: 1000,
            outline_rasterization_mode: OutlineRasterizationMode::Msdf,
        }
    }
}
impl BrowserTextRasterSettings {
    fn from_fonts_handle(
        fonts_rc: &Rc<RefCell<makepad_draw::text::fonts::Fonts>>,
    ) -> Self {
        let fonts = fonts_rc.borrow();
        let rasterizer = fonts.rasterizer().borrow();
        let sdfer = rasterizer.sdfer().settings();
        let msdfer = rasterizer.msdfer().settings();
        let msdf_resolution = rasterizer.msdf_resolution();
        let msdf_complexity = rasterizer.msdf_complexity();
        Self {
            page_size: rasterizer.color_atlas().size(),
            sdfer_padding: sdfer.padding,
            sdfer_radius: sdfer.radius,
            sdfer_cutoff: sdfer.cutoff,
            msdf_padding: msdfer.padding,
            msdf_radius: msdfer.radius,
            msdf_cutoff: msdfer.cutoff,
            msdf_corner_angle_threshold: msdfer.corner_angle_threshold,
            min_request_dpxs_per_em: msdf_resolution.min_request_dpxs_per_em,
            min_dpxs_per_em: msdf_resolution.min_dpxs_per_em,
            max_dpxs_per_em: msdf_resolution.max_dpxs_per_em,
            dpx_quantum: msdf_resolution.dpx_quantum,
            max_outline_commands: msdf_complexity.max_outline_commands,
            max_estimated_segments: msdf_complexity.max_estimated_segments,
            outline_rasterization_mode: rasterizer.outline_rasterization_mode(),
        }
    }

    fn sdfer_settings(self) -> sdfer::Settings {
        sdfer::Settings {
            padding: self.sdfer_padding,
            radius: self.sdfer_radius,
            cutoff: self.sdfer_cutoff,
        }
    }

    fn msdf_settings(self) -> msdfer::Settings {
        msdfer::Settings {
            padding: self.msdf_padding,
            radius: self.msdf_radius,
            cutoff: self.msdf_cutoff,
            corner_angle_threshold: self.msdf_corner_angle_threshold,
        }
    }

    fn msdf_complexity_settings(self) -> MsdfComplexitySettings {
        MsdfComplexitySettings {
            max_outline_commands: self.max_outline_commands,
            max_estimated_segments: self.max_estimated_segments,
        }
    }
}


fn glyph_outline_image_size(size_in_ems: Size<f32>, dpxs_per_em: f32) -> Size<usize> {
    let size_in_dpxs = size_in_ems * dpxs_per_em;
    Size::new(
        size_in_dpxs.width.ceil() as usize,
        size_in_dpxs.height.ceil() as usize,
    )
}

fn quantize_outline_dpx(
    requested_dpxs_per_em: f32,
    settings: BrowserTextRasterSettings,
    format: BrowserGlyphFormat,
) -> f32 {
    let min_dpx = match format {
        BrowserGlyphFormat::Alpha | BrowserGlyphFormat::Msdf => settings.min_dpxs_per_em,
        BrowserGlyphFormat::Color => 1.0,
    };
    let clamped = requested_dpxs_per_em
        .max(min_dpx)
        .min(settings.max_dpxs_per_em.max(min_dpx));
    let quantum = settings.dpx_quantum.max(1.0);
    (clamped / quantum).round() * quantum
}

fn quantize_color_dpx(requested_dpxs_per_em: f32) -> f32 {
    requested_dpxs_per_em.max(1.0).round().min(u16::MAX as f32)
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

fn next_page_id(next_page_id: &mut u32) -> BrowserGlyphAtlasPageId {
    let id = BrowserGlyphAtlasPageId(*next_page_id);
    *next_page_id = next_page_id.wrapping_add(1);
    id
}

fn next_generation(next_generation: &mut u64) -> u64 {
    let generation = *next_generation;
    *next_generation = next_generation.wrapping_add(1);
    generation
}

fn scene_contains_text_run(
    scene: &MpBrowserScene,
    stable_id: crate::browser_scene::MpBrowserStableTextRunId,
) -> bool {
    scene.text_runs.iter().any(|run| run.stable_id == stable_id)
        || scene.tasks.iter().any(|task| match &task.kind {
            MpBrowserTaskKind::Scene(task_scene) => scene_contains_text_run(task_scene, stable_id),
            MpBrowserTaskKind::Blur { .. } => false,
        })
}

