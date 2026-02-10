use super::{
    font::{Font, GlyphId},
    font_atlas::{
        ColorAtlas, GlyphImage, GlyphImageKey, GlyphImageKind, GrayscaleAtlas, MsdfAtlas,
    },
    geom::{Point, Rect, Size},
    glyph_outline::{Command, GlyphOutline},
    image::{Bgra, Image, R},
    msdfer,
    msdfer::Msdfer,
    sdfer,
    sdfer::Sdfer,
};
use std::collections::HashSet;
//use std::{fs::File, io::BufWriter, path::Path, slice};

#[derive(Debug)]
pub struct Rasterizer {
    sdfer: Sdfer,
    msdfer: Msdfer,
    msdf_resolution: MsdfResolutionSettings,
    msdf_complexity: MsdfComplexitySettings,
    outline_rasterization_mode: OutlineRasterizationMode,
    atlas: ColorAtlas,
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
        let Some(rect) = self.atlas.get_cached_glyph_image_rect(&job.key) else {
            return;
        };
        if job.pixels.len() != job.key.size.width.saturating_mul(job.key.size.height) {
            return;
        }
        if !self.is_valid_msdf_upgrade(rect, &job.pixels) {
            self.outline_msdf_failed.insert(job.key);
            return;
        }

        {
            let mut dst = self.atlas.get_cached_glyph_image_mut(rect);
            for y in 0..job.key.size.height {
                for x in 0..job.key.size.width {
                    dst[Point::new(x, y)] = job.pixels[y * job.key.size.width + x];
                }
            }
        }
        self.outline_msdf_ready.insert(job.key);
    }

    fn is_valid_msdf_upgrade(&self, rect: Rect<usize>, msdf_pixels: &[Bgra]) -> bool {
        let sdf_threshold = 1.0 - self.sdfer.settings().cutoff;
        let padding = self.sdfer.settings().padding;
        let width = rect.size.width;
        let height = rect.size.height;
        if width <= padding * 2 || height <= padding * 2 {
            return true;
        }

        let atlas = self.atlas.image();
        let mut mismatch = 0usize;
        let mut checked = 0usize;
        let sx = 8usize;
        let sy = 8usize;
        for gy in 0..sy {
            let y = padding + gy * (height - padding * 2 - 1) / (sy - 1);
            for gx in 0..sx {
                let x = padding + gx * (width - padding * 2 - 1) / (sx - 1);
                let local_index = y * width + x;
                let old = atlas[rect.origin + Size::new(x, y)];
                let old_sdf = old.r() as f32 / 255.0;
                let new_msdf = msdf_median(msdf_pixels[local_index]);
                if (old_sdf - sdf_threshold).abs() < 0.03 || (new_msdf - sdf_threshold).abs() < 0.03
                {
                    continue;
                }
                checked += 1;
                let old_inside = old_sdf > sdf_threshold;
                let new_inside = new_msdf > sdf_threshold;
                if old_inside != new_inside {
                    mismatch += 1;
                }
            }
        }

        if checked == 0 {
            return true;
        }
        mismatch * 100 <= checked * 35
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
            kind: GlyphImageKind::Outline,
        };
        let atlas_image_bounds = match self.atlas.get_or_allocate_glyph_image(key.clone())? {
            GlyphImage::Cached(rect) => rect,
            GlyphImage::Allocated(mut slot) => {
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
                        slot[Point::new(x, y)] = Bgra::new(v, v, v, v);
                    }
                }
                self.outline_msdf_ready.remove(&key);
                self.outline_msdf_failed.remove(&key);
                slot.bounds()
            }
        };

        return Some(RasterizedGlyph {
            atlas_kind: AtlasKind::Grayscale,
            atlas_size: self.atlas.size(),
            atlas_image_bounds,
            atlas_image_padding,
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
        let mut outline = None;
        let bounds_in_ems = font.glyph_outline_bounds_in_ems(glyph_id, &mut outline)?;
        let outline = outline.unwrap_or_else(|| font.glyph_outline(glyph_id).unwrap());
        let complexity = estimate_outline_complexity(&outline);
        if !is_msdf_complexity_acceptable(self.msdf_complexity, complexity) {
            return self.rasterize_glyph_outline_sdf(font, glyph_id, dpxs_per_em);
        }
        let dpxs_per_em = choose_msdf_resolution(self.msdf_resolution, &outline, dpxs_per_em);
        let atlas_image_size = glyph_outline_image_size(bounds_in_ems.size, dpxs_per_em);
        let atlas_image_padding = self.msdfer.settings().padding;
        let key = GlyphImageKey {
            font_id: font.id(),
            glyph_id,
            size: atlas_image_size + Size::from(atlas_image_padding) * 2,
            kind: GlyphImageKind::Outline,
        };
        if self.outline_msdf_failed.contains(&key) {
            return self.rasterize_glyph_outline_sdf(font, glyph_id, dpxs_per_em);
        }
        if self.outline_msdf_ready.contains(&key) {
            if let Some(atlas_image_bounds) = self.atlas.get_cached_glyph_image_rect(&key) {
                return Some(RasterizedGlyph {
                    atlas_kind: AtlasKind::Msdf,
                    atlas_size: self.atlas.size(),
                    atlas_image_bounds,
                    atlas_image_padding,
                    origin_in_dpxs: bounds_in_ems.origin * dpxs_per_em,
                    dpxs_per_em,
                });
            }
            self.outline_msdf_ready.remove(&key);
        }

        let sdf_glyph = self.rasterize_glyph_outline_sdf(font, glyph_id, dpxs_per_em)?;
        if !self.outline_msdf_ready.contains(&key) && self.outline_msdf_pending.insert(key.clone())
        {
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
        let atlas_image_bounds = match self.atlas.get_or_allocate_glyph_image(GlyphImageKey {
            font_id: font.id(),
            glyph_id,
            size: raster_image.decode_size() + Size::from(2 * PADDING),
            kind: GlyphImageKind::Color,
        })? {
            GlyphImage::Cached(rect) => rect,
            GlyphImage::Allocated(mut image) => {
                let size = image.size();
                image = image.subimage_mut(Rect::from(size).unpad(PADDING));
                raster_image.decode(&mut image);
                image.bounds()
            }
        };
        return Some(RasterizedGlyph {
            atlas_kind: AtlasKind::Color,
            atlas_size: self.atlas.size(),
            atlas_image_bounds,
            atlas_image_padding: PADDING,
            origin_in_dpxs: raster_image.origin_in_dpxs(),
            dpxs_per_em: raster_image.dpxs_per_em(),
        });
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
    pub origin_in_dpxs: Point<f32>,
    pub dpxs_per_em: f32,
}

#[derive(Clone, Copy, Debug)]
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

fn msdf_median(pixel: Bgra) -> f32 {
    let r = pixel.r() as f32 / 255.0;
    let g = pixel.g() as f32 / 255.0;
    let b = pixel.b() as f32 / 255.0;
    (r + g + b) - r.min(g).min(b) - r.max(g).max(b)
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
