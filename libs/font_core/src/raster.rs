//! Glyph rasterisation shared by the text layout and the HAL's font service: the settings, the
//! atlas rectangle packer, and the pure steps that turn a glyph outline (or an embedded bitmap)
//! into pixels. A *plan* is the cheap part (size, atlas key, metrics, from the outline's bounds);
//! rendering it is the expensive part, so a caller that finds the key in its atlas never pays it.

use {
    super::{
        font::{Font, GlyphId},
        font_atlas::{GlyphImageKey, GlyphImageKind},
        geom::{Point, Rect, Size},
        glyph_outline::{GlyphOutline, OutlineComplexity},
        image::{Bgra, Image, SubimageMut, R},
        msdfer::{self, Msdfer},
        sdfer::{self, Sdfer},
    },
    std::rc::Rc,
};

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub sdfer: sdfer::Settings,
    pub msdfer: msdfer::Settings,
    pub msdf_resolution: MsdfResolutionSettings,
    pub msdf_complexity: MsdfComplexitySettings,
    pub outline_rasterization_mode: OutlineRasterizationMode,
    pub atlas_size: Size<usize>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sdfer: sdfer::Settings {
                padding: 4,
                radius: 8.0,
                cutoff: 0.25,
            },
            msdfer: msdfer::Settings {
                padding: 4,
                radius: 8.0,
                cutoff: 0.25,
                corner_angle_threshold: 3.0,
            },
            msdf_resolution: MsdfResolutionSettings {
                min_request_dpxs_per_em: 20.0,
                min_dpxs_per_em: 32.0,
                base_dpxs_per_em: 64.0,
                max_dpxs_per_em: 128.0,
                target_feature_texels: 1.75,
                dpx_quantum: 8.0,
                min_feature_floor_ems: 1.0 / 1024.0,
            },
            msdf_complexity: MsdfComplexitySettings {
                max_outline_commands: 180,
                max_estimated_segments: 1000,
            },
            outline_rasterization_mode: OutlineRasterizationMode::Msdf,
            atlas_size: default_text_atlas_size(),
        }
    }
}

/// The glyph atlas size: 2048 x 2048 unless `MAKEPAD_TEXT_ATLAS_SIZE` (`1024` or `1024x2048`)
/// names another.
pub fn default_text_atlas_size() -> Size<usize> {
    std::env::var("MAKEPAD_TEXT_ATLAS_SIZE")
        .ok()
        .and_then(|raw| parse_text_atlas_size_value(raw.trim()))
        .unwrap_or_else(|| Size::new(2048, 2048))
}

pub fn parse_text_atlas_size_value(value: &str) -> Option<Size<usize>> {
    if value.is_empty() {
        return None;
    }

    fn parse_dim(dim: &str) -> Option<usize> {
        let parsed = dim.trim().parse::<usize>().ok()?;
        if (256..=8192).contains(&parsed) {
            Some(parsed)
        } else {
            None
        }
    }

    if let Some((w, h)) = value.split_once('x').or_else(|| value.split_once('X')) {
        return Some(Size::new(parse_dim(w)?, parse_dim(h)?));
    }

    let dim = parse_dim(value)?;
    Some(Size::new(dim, dim))
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
    /// What this glyph is (font, glyph, kind, size) when its pixels live in an atlas that is not
    /// this process's: the atlas rectangle above is a snapshot of an answer that can still arrive
    /// (`Unresolved`), be upgraded or be taken back. A draw from a kept glyph looks it up again
    /// through this key. Default (font 0) = an image this process owns; nothing to look up.
    pub source: GlyphSource,
}

/// The key a lent-atlas glyph is looked up by again at draw time (see `RasterizedGlyph::source`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct GlyphSource {
    /// the session's font id; 0 = none
    pub font: u32,
    pub glyph: u16,
    /// the request kind (the HAL font wire's `kind`)
    pub kind: u32,
    /// the requested pixels per em, as bits
    pub dpxs_bits: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AtlasKind {
    Grayscale,
    Color,
    Msdf,
    /// Valid glyph metrics whose pixels have not arrived from the HAL yet. The draw samples
    /// `UNRESOLVED_RASTER_RECT`, independently of the glyph's image bounds.
    Unresolved,
}

/// The HAL reserves this transparent texel in every plane, including after atlas reclamation.
pub const UNRESOLVED_RASTER_RECT: Rect<usize> = Rect::new(Point::new(0, 0), Size::new(1, 1));

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OutlineRasterizationMode {
    Sdf,
    Msdf,
}

pub fn glyph_outline_image_size(size_in_ems: Size<f32>, dpxs_per_em: f32) -> Size<usize> {
    let size_in_dpxs = size_in_ems * dpxs_per_em;
    Size::new(
        size_in_dpxs.width.ceil() as usize,
        size_in_dpxs.height.ceil() as usize,
    )
}

pub fn is_msdf_complexity_acceptable(
    settings: MsdfComplexitySettings,
    complexity: OutlineComplexity,
) -> bool {
    complexity.outline_commands <= settings.max_outline_commands
        && complexity.estimated_segments <= settings.max_estimated_segments
}

// ---- plans: what rasterising a glyph needs, known from the outline's bounds alone

/// A glyph outline as a signed distance field: the atlas key and metrics of the result, and the
/// outline to render it from.
pub struct SdfPlan {
    pub key: GlyphImageKey,
    /// the coverage image (without padding)
    pub image_size: Size<usize>,
    pub padding: usize,
    pub origin_in_dpxs: Point<f32>,
    pub dpxs_per_em: f32,
    outline: Rc<GlyphOutline>,
}

impl SdfPlan {
    /// The distance field, `key.size` pixels (the coverage plus the padding all around).
    pub fn render(&self, sdfer: &mut Sdfer) -> Image<R> {
        let mut coverage = Image::new(self.image_size);
        self.outline
            .rasterize(self.dpxs_per_em, &mut coverage.subimage_mut(self.image_size.into()));
        let size = self.image_size + Size::from(self.padding) * 2;
        let mut sdf = Image::<R>::new(size);
        sdfer.coverage_to_sdf(
            &coverage.subimage(self.image_size.into()),
            &mut sdf.subimage_mut(Rect::from(size)),
        );
        sdf
    }
}

pub fn plan_outline_sdf(
    font: &Font,
    glyph_id: GlyphId,
    mut dpxs_per_em: f32,
    settings: &Settings,
) -> Option<SdfPlan> {
    debug_assert_eq!(settings.sdfer.padding, settings.msdfer.padding);
    dpxs_per_em = dpxs_per_em.max(settings.msdf_resolution.min_dpxs_per_em);
    let mut outline = None;
    let bounds_in_ems = font.glyph_outline_bounds_in_ems(glyph_id, &mut outline)?;
    let outline = outline.unwrap_or_else(|| font.glyph_outline_rc(glyph_id).unwrap());
    let image_size = glyph_outline_image_size(bounds_in_ems.size, dpxs_per_em);
    let padding = settings.sdfer.padding;
    Some(SdfPlan {
        key: GlyphImageKey {
            font_id: font.id(),
            glyph_id,
            size: image_size + Size::from(padding) * 2,
            kind: GlyphImageKind::OutlineSdf,
        },
        image_size,
        padding,
        origin_in_dpxs: bounds_in_ems.origin * dpxs_per_em,
        dpxs_per_em,
        outline,
    })
}

/// A glyph outline as a multi-channel distance field. Small text, and outlines too complex for
/// the MSDF generator, have no plan: they use the plain distance field.
pub struct MsdfPlan {
    pub key: GlyphImageKey,
    pub padding: usize,
    pub origin_in_dpxs: Point<f32>,
    pub dpxs_per_em: f32,
    outline: Rc<GlyphOutline>,
}

impl MsdfPlan {
    /// A copy of the outline for a thread that renders it.
    pub fn outline(&self) -> GlyphOutline {
        (*self.outline).clone()
    }
}

pub fn plan_outline_msdf(
    font: &Font,
    glyph_id: GlyphId,
    dpxs_per_em: f32,
    settings: &Settings,
) -> Option<MsdfPlan> {
    // Always keep small text on SDF, even if an MSDF slot already exists.
    if dpxs_per_em <= settings.msdf_resolution.min_request_dpxs_per_em {
        return None;
    }
    let mut outline = None;
    let bounds_in_ems = font.glyph_outline_bounds_in_ems(glyph_id, &mut outline)?;
    let outline = outline.unwrap_or_else(|| font.glyph_outline_rc(glyph_id).unwrap());
    if !is_msdf_complexity_acceptable(settings.msdf_complexity, outline.complexity()) {
        return None;
    }
    let dpxs_per_em = dpxs_per_em.max(settings.msdf_resolution.min_dpxs_per_em);
    let image_size = glyph_outline_image_size(bounds_in_ems.size, dpxs_per_em);
    let padding = settings.msdfer.padding;
    Some(MsdfPlan {
        key: GlyphImageKey {
            font_id: font.id(),
            glyph_id,
            size: image_size + Size::from(padding) * 2,
            kind: GlyphImageKind::OutlineMsdf,
        },
        padding,
        origin_in_dpxs: bounds_in_ems.origin * dpxs_per_em,
        dpxs_per_em,
        outline,
    })
}

/// Renders the multi-channel distance field of `outline`, `size` pixels.
pub fn render_msdf(msdfer: &mut Msdfer, outline: &GlyphOutline, dpxs_per_em: f32, size: Size<usize>) -> Image<Bgra> {
    let mut msdf = Image::<Bgra>::new(size);
    msdfer.outline_to_msdf(outline, dpxs_per_em, &mut msdf.subimage_mut(Rect::from(size)));
    msdf
}

/// An embedded colour bitmap of a glyph, scaled toward the display size.
pub struct ColorPlan {
    pub key: GlyphImageKey,
    /// the bitmap (without padding)
    pub target: Size<usize>,
    pub origin_in_dpxs: Point<f32>,
    pub dpxs_per_em: f32,
}

pub const COLOR_PADDING: usize = 2;

pub fn plan_color(font: &Font, glyph_id: GlyphId, dpxs_per_em: f32) -> Option<ColorPlan> {
    // Keep a little extra resolution above the display size so minor magnification
    // (line scaling, fractional DPI) stays crisp without re-introducing aliasing.
    const HEADROOM: f32 = 1.5;

    font.with_glyph_raster_image(glyph_id, dpxs_per_em, |raster_image| {
        let native = raster_image.decode_size();
        let native_dpxs = raster_image.dpxs_per_em();

        // Color emoji ship at one large native strike; pre-shrink toward display size so it
        // isn't minified (no mipmaps) into a blocky look. Never upscale.
        let scale = if native_dpxs > 0.0 {
            (dpxs_per_em * HEADROOM / native_dpxs).clamp(0.0, 1.0)
        } else {
            1.0
        };
        // Quantize the scale into coarse buckets (powers of 1.25, rounded up toward
        // native). The atlas slot and the strike decode below are keyed by the
        // resulting size, so without this a continuous zoom or a fractional-DPI
        // change re-decodes the full strike and allocates a fresh slot for every
        // sub-pixel size step. Rounding up keeps at least the requested resolution,
        // and the achieved-scale compensation below keeps the on-screen size exact.
        const SIZE_BUCKET_FACTOR: f32 = 1.25;
        let scale = if scale > 0.0 && scale < 1.0 {
            let steps = ((1.0 / scale).ln() / SIZE_BUCKET_FACTOR.ln() + 1.0e-3).floor();
            (1.0 / SIZE_BUCKET_FACTOR.powf(steps)).min(1.0)
        } else {
            scale
        };
        let target = if scale < 1.0 {
            Size::new(
                ((native.width as f32 * scale).round() as usize).max(1),
                ((native.height as f32 * scale).round() as usize).max(1),
            )
        } else {
            native
        };
        // Scale actually achieved after integer rounding. Scaling both origin_in_dpxs
        // and dpxs_per_em by it keeps the glyph's on-screen size & position invariant
        // (the draw side works in em units = *_in_dpxs / dpxs_per_em).
        //
        // width and height are rounded independently above, so their ratios can differ
        // slightly; we apply one uniform scale (origin is scaled on both axes and
        // dpxs_per_em is a scalar), so use the geometric mean of the per-axis ratios to
        // spread the sub-pixel rounding error evenly instead of distorting one axis.
        let achieved_w = if native.width > 0 {
            target.width as f32 / native.width as f32
        } else {
            1.0
        };
        let achieved_h = if native.height > 0 {
            target.height as f32 / native.height as f32
        } else {
            1.0
        };
        let achieved = (achieved_w * achieved_h).sqrt();

        Some(ColorPlan {
            key: GlyphImageKey {
                font_id: font.id(),
                glyph_id,
                size: target + Size::from(2 * COLOR_PADDING),
                kind: GlyphImageKind::Color,
            },
            target,
            origin_in_dpxs: raster_image.origin_in_dpxs() * achieved,
            dpxs_per_em: native_dpxs * achieved,
        })
    })?
}

impl ColorPlan {
    /// Decodes the bitmap into `out` (`key.size` pixels: the bitmap inside the padding).
    pub fn render(&self, font: &Font, glyph_id: GlyphId, dpxs_per_em: f32, out: SubimageMut<Bgra>) {
        font.with_glyph_raster_image(glyph_id, dpxs_per_em, |raster_image| {
            let size = out.size();
            let mut inner = out.subimage_mut(Rect::from(size).unpad(COLOR_PADDING));
            raster_image.decode_scaled(&mut inner);
        });
    }
}

// ---- the atlas rectangle packer

#[derive(Clone, Copy, Debug)]
pub struct AtlasSlot {
    pub rect: Rect<usize>,
    pub plane: AtlasPlane,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AtlasPlane {
    R,
    G,
    B,
    A,
}

impl AtlasPlane {
    pub fn from_index(index: usize) -> Self {
        match index & 3 {
            0 => Self::R,
            1 => Self::G,
            2 => Self::B,
            _ => Self::A,
        }
    }

    pub fn index(self) -> u8 {
        match self {
            Self::R => 0,
            Self::G => 1,
            Self::B => 2,
            Self::A => 3,
        }
    }

    pub fn set(self, pixel: Bgra, value: u8) -> Bgra {
        match self {
            Self::R => Bgra::new(pixel.b(), pixel.g(), value, pixel.a()),
            Self::G => Bgra::new(pixel.b(), value, pixel.r(), pixel.a()),
            Self::B => Bgra::new(value, pixel.g(), pixel.r(), pixel.a()),
            Self::A => Bgra::new(pixel.b(), pixel.g(), pixel.r(), value),
        }
    }

    pub fn get(self, pixel: Bgra) -> u8 {
        match self {
            Self::R => pixel.r(),
            Self::G => pixel.g(),
            Self::B => pixel.b(),
            Self::A => pixel.a(),
        }
    }
}

#[derive(Debug)]
pub struct MultiPlaneAllocator {
    planes: [RectPacker; 4],
    plane_used_area: [usize; 4],
    round_robin_cursor: usize,
}

impl MultiPlaneAllocator {
    pub fn new(size: Size<usize>) -> Self {
        Self {
            planes: std::array::from_fn(|_| RectPacker::new(size)),
            plane_used_area: [0; 4],
            round_robin_cursor: 0,
        }
    }

    pub fn reset(&mut self, size: Size<usize>) {
        self.planes = std::array::from_fn(|_| RectPacker::new(size));
        self.plane_used_area = [0; 4];
        self.round_robin_cursor = 0;
    }

    /// Starts over with exactly `used` taken: a slot of one plane takes its rectangle in that
    /// plane, a shared slot (`None`) in all four. Used after an eviction; false when one does
    /// not fit (the allocator then holds what fitted before it).
    pub fn rebuild(&mut self, size: Size<usize>, used: impl IntoIterator<Item = (Rect<usize>, Option<AtlasPlane>)>) -> bool {
        self.reset(size);
        let mut all = true;
        for (rect, plane) in used {
            let area = rect.size.width.saturating_mul(rect.size.height);
            match plane {
                Some(plane) => {
                    let i = plane.index() as usize;
                    all &= self.planes[i].reserve(rect);
                    self.plane_used_area[i] = self.plane_used_area[i].saturating_add(area);
                }
                None => {
                    for (plane, used_area) in self.planes.iter_mut().zip(&mut self.plane_used_area) {
                        all &= plane.reserve(rect);
                        *used_area = used_area.saturating_add(area);
                    }
                }
            }
        }
        all
    }

    pub fn allocate_sdf_slot(&mut self, size: Size<usize>) -> Option<AtlasSlot> {
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

    pub fn allocate_shared_slot(&mut self, size: Size<usize>) -> Option<Rect<usize>> {
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
pub struct RectPacker {
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

#[cfg(test)]
mod tests {
    use super::{parse_text_atlas_size_value, Size};

    #[test]
    fn parses_text_atlas_size_from_env_value() {
        assert_eq!(
            parse_text_atlas_size_value("1024"),
            Some(Size::new(1024, 1024))
        );
        assert_eq!(
            parse_text_atlas_size_value("1024x2048"),
            Some(Size::new(1024, 2048))
        );
        assert_eq!(
            parse_text_atlas_size_value("1024X2048"),
            Some(Size::new(1024, 2048))
        );
        assert_eq!(parse_text_atlas_size_value(""), None);
        assert_eq!(parse_text_atlas_size_value("64"), None);
        assert_eq!(parse_text_atlas_size_value("bogus"), None);
    }
}
