//! Text clips like quads: per-instance clip ∩ list clip after the view transform.
//!
//! As in `DrawQuad::clip_and_transform_vertex`, `view_clip` is expressed in
//! list coordinates (callers inverse-transform a screen clip), and applies
//! after `view_shift`, before the matrix transform. All text vertex variants
//! use that same intersection. Sampling uses the clipped vertex translated
//! back by `view_shift`, relative to the original glyph rectangle; clipping
//! crops glyphs without moving or stretching their atlas/curve coordinates.

use {
    crate::{
        cx_2d::Cx2d,
        cx_draw::CxDraw,
        draw_list_2d::ManyInstances,
        makepad_platform::*,
        makepad_platform::recording_buffer::RecordingBuffer,
        text::{
            color::Color,
            font::FontId,
            font_family::{FontDiagnostics, FontFamilyId},
            fonts::Fonts,
            geom::{Point, Rect as TextRect, Size, Transform},
            layouter::{
                BorrowedLayoutParams, LaidoutGlyph, LaidoutRow, LaidoutText, LayoutOptions, Style,
            },
            loader::{FontDefinition, FontFamilyDefinition},
            rasterizer::{AtlasKind, RasterizedGlyph},
            slug_atlas::SlugGlyphCacheResult,
        },
        turtle::*,
        turtle::{Align, Walk},
    },
    std::{
        cell::RefCell,
        collections::hash_map::DefaultHasher,
        hash::{Hash, Hasher},
        rc::Rc,
    },
};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom
    use mod.res.*

    mod.text = {
        let text = me
        FontFamily: mod.std.set_type_default() do #(FontFamily::script_component(vm))
        FontMember: mod.std.set_type_default() do #(FontMember::script_api(vm))
        TextOverflow: mod.std.set_type_default() do #(TextOverflow::script_api(vm)),
        ..me.TextOverflow,
        TextStyle: mod.std.set_type_default() do #(TextStyle::script_api(vm)){
            font_size: 10
            font_family: text.FontFamily{
                latin := text.FontMember{res: crate_resource("self:../widgets/resources/IBMPlexSans-Text.ttf") asc:-0.1 desc:0.0}
            }
            line_spacing: 1.2
        }
    }

    use mod.text.*

    mod.draw.DrawText = mod.std.set_type_default() do #(DrawText::script_shader(vm)){
        ..mod.glyph_coverage.SlugCoverage,
        ..mod.glyph_coverage.AtlasCoverage


        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)

        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)

        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)

        color: #fff
        sdf_sharpness: 1.0
        sdf_luma_bias: 0.03

        pos: varying(vec2f)
        t: varying(vec2f)
        world: varying(vec4f)

        radius: uniform(float)
        cutoff: uniform(float)
        aa_pad_px: uniform(float(1.0))
        slug_viewport_px: uniform(vec2(1.0, 1.0))
        total_chars: instance(1000000.0)

        grayscale_texture: texture_2d(float)
        color_texture: texture_2d(float)
        msdf_texture: texture_2d(float)
        curve_texture: texture_2d(float)
        band_texture: texture_2d(float)



        vertex: fn() {
            let use_slug = if self.texture_index > 2.5 {1.0} else {0.0}

            let p_raster = mix(self.rect_pos, self.rect_pos + self.rect_size, self.geom.pos)
            let p_clipped_raster = clamp(
                clamp(p_raster, self.draw_clip.xy, self.draw_clip.zw) + self.draw_list.view_shift,
                self.draw_list.view_clip.xy,
                self.draw_list.view_clip.zw
            )
            let p_normalized_raster = (p_clipped_raster - self.draw_list.view_shift - self.rect_pos) / self.rect_size

            let pad_lpx = self.aa_pad_px / max(self.draw_pass.dpi_factor, 0.0001)
            let content_rect_pos = self.rect_pos + vec2(pad_lpx, pad_lpx)
            let content_rect_size = vec2(
                max(self.rect_size.x - 2.0 * pad_lpx, 0.0001),
                max(self.rect_size.y - 2.0 * pad_lpx, 0.0001)
            )
            let p_slug = mix(content_rect_pos, content_rect_pos + content_rect_size, self.geom.pos)
            let jac = vec4(1.0 / content_rect_size.x, 0.0, 0.0, 1.0 / content_rect_size.y)
            let corner = self.geom.pos * 2.0 - 1.0
            let normal = if dot(corner, corner) > 0.000001 {
                corner
            } else {
                vec2(1.0, 0.0)
            }
            let dilated = self.slug_dilate(p_slug, self.geom.pos, jac, normal)
            let p_clipped_slug = clamp(
                clamp(dilated.zw, self.draw_clip.xy, self.draw_clip.zw) + self.draw_list.view_shift,
                self.draw_list.view_clip.xy,
                self.draw_list.view_clip.zw
            )
            let p_sample_slug = p_clipped_slug - self.draw_list.view_shift
            let pos_slug = vec2(
                dilated.x + (p_sample_slug.x - dilated.z) * jac.x,
                dilated.y + (p_sample_slug.y - dilated.w) * jac.w
            )

            self.pos = mix(p_normalized_raster, pos_slug, use_slug)
            self.t = mix(self.t_min, self.t_max, p_normalized_raster.xy)
            let final_pos = mix(p_clipped_raster, p_clipped_slug, use_slug)
            self.world = self.draw_list.view_transform * vec4(
                final_pos.x,
                final_pos.y,
                self.glyph_depth + self.draw_call.zbias,
                1.
            )
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }






        slug_curve_offset: fn() -> float {
            return self.t_min.x
        }

        slug_curve_count: fn() -> float {
            return self.t_min.y
        }

        slug_band_offset: fn() -> float {
            return self.t_max.x
        }

        slug_band_count: fn() -> float {
            return self.t_max.y
        }

        slug_fill_flags: fn() -> float {
            return self.atlas_plane
        }










        sample_slug_pixel: fn() {
            if self.slug_curve_count() < 0.5 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }

            let sample = self.pos
            let px_x = max(abs(dFdx(sample.x)) + abs(dFdy(sample.x)), 0.00001)
            let px_y = max(abs(dFdx(sample.y)) + abs(dFdy(sample.y)), 0.00001)
            // One loop over the supersample grid (4x4, 2x2, or the pixel centre),
            // so alpha_at and its four curve-scan loops are inlined once. Unrolled
            // into 21 calls they made Adreno 630's compiler spend ~3.5 s on each
            // text program. Offsets match the unrolled ones: (i + 0.5) / grid - 0.5 px.
            let grid = if self.aa_4x4 > 0.5 {4.0} else if self.aa_2x2 > 0.5 {2.0} else {1.0}
            // 1/grid is exact (1, 1/2, 1/4): offsets and the average round
            // exactly as the unrolled sums did, and the row index is exact
            // (no division, which some GPUs approximate).
            let inv_grid = if self.aa_4x4 > 0.5 {0.25} else if self.aa_2x2 > 0.5 {0.5} else {1.0}
            var alpha_sum = 0.0
            var sample_index = 0.0
            loop {
                if sample_index >= grid * grid { break }
                let row = floor(sample_index * inv_grid)
                let col = sample_index - row * grid
                let offset = (vec2(col, row) + 0.5 - grid * 0.5) * inv_grid * vec2(px_x, px_y)
                alpha_sum = alpha_sum + self.alpha_at(sample + offset, px_x, px_y)
                sample_index = sample_index + 1.0
            }
            let alpha_base = clamp(alpha_sum * inv_grid * inv_grid, 0.0, 1.0)
            let darken = clamp(max(px_x, px_y) * self.stem_darken, 0.0, self.stem_darken_max)
            let edge_weight = clamp(1.0 - abs(alpha_base * 2.0 - 1.0), 0.0, 1.0)
            let alpha = clamp(alpha_base + darken * edge_weight, 0.0, 1.0)
            let color = self.get_color()
            return vec4(color.rgb * color.a * alpha, color.a * alpha)
        }

        get_color: fn() {
            return self.color
        }

        alpha_at: fn(sample: vec2, px_x: float, px_y: float) -> float {
            return self.alpha_at_scan(sample, px_x, px_y)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip);
        }

        sample_text_pixel: fn() {
            if self.texture_index < -0.5 {
                return self.color_texture.sample(vec2(0.5, 0.5) / self.color_texture.size())
            }
            if self.texture_index < 0.5 {
                let dxt = length(dFdx(self.t))
                let dyt = length(dFdy(self.t))
                let c = self.get_color()
                let scale = (dxt + dyt) * self.grayscale_texture.size().x * 0.5
                let tex_size = self.grayscale_texture.size()
                let half_texel = vec2(0.5 / tex_size.x, 0.5 / tex_size.y)
                let p = clamp(self.t.xy, self.t_min + half_texel, self.t_max - half_texel)
                let s = self.sdf(scale, p, c)
                return s * vec4(c.rgb * c.a, c.a)
            } else if self.texture_index < 1.5 {
                let tex_size = self.color_texture.size()
                let half_texel = vec2(0.5 / tex_size.x, 0.5 / tex_size.y)
                let p = clamp(self.t.xy, self.t_min + half_texel, self.t_max - half_texel)
                let c = self.color_texture.sample(p)
                return vec4(c.rgb * c.a, c.a)
            } else if self.texture_index < 2.5 {
                let dxt = length(dFdx(self.t))
                let dyt = length(dFdy(self.t))
                let c = self.get_color()
                let scale = (dxt + dyt) * self.msdf_texture.size().x * 0.5
                let tex_size = self.msdf_texture.size()
                let half_texel = vec2(0.5 / tex_size.x, 0.5 / tex_size.y)
                let p = clamp(self.t.xy, self.t_min + half_texel, self.t_max - half_texel)
                let s = self.msdf(scale, p, c)
                return s * vec4(c.rgb * c.a, c.a)
            } else {
                return self.sample_slug_pixel()
            }
        }

        pixel: fn() {
            return self.sample_text_pixel()
        }
    }
}

/// Controls how text overflow is handled when text exceeds its container.
///
/// Analogous to CSS `text-overflow`. Requires a width constraint to take effect.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook)]
pub enum TextOverflow {
    /// Text is clipped at the container boundary (default).
    #[pick]
    Clip,
    /// An ellipsis character (U+2026 "…") is shown where text is truncated.
    Ellipsis,
}

#[derive(Default)]
#[repr(align(16))]
struct DrawTextInstanceAlign;

#[derive(Script)]
#[repr(C, align(16))]
pub struct DrawText {
    #[rust]
    pub many_instances: Option<ManyInstances>,
    #[rust]
    pending_slug_flush_generation: u64,
    #[rust]
    slug_flush_defer_depth: u64,
    #[live]
    pub text_style: TextStyle,
    #[live(1.0)]
    pub font_scale: f32,
    #[live(0.0)]
    pub draw_depth: f32,
    #[live]
    pub debug: bool,

    #[live]
    pub temp_y_shift: f32,

    /// Center the text's *ink* in the box a walk allocates for it, rather than
    /// its line box.
    ///
    /// A widget that puts one line of text in a box it paints — a button face,
    /// a tab, a dropdown — wants the cap-height band centered, because that
    /// band is what the eye reads as "the text". Centering the line box
    /// instead leaves the text sitting high by half the difference between the
    /// font's over-cap ascender and its descender. Off by default: it moves
    /// glyphs relative to their box, which text that has to line up with
    /// something else (a code editor's rows, a text flow's baselines) must not
    /// do. Only [`DrawText::draw_walk`] honors it; the resumable flow paths
    /// stay baseline-exact.
    #[live]
    pub ink_centered: bool,

    /// Centering height (in unscaled lpxs) for RowAlign::Center; TextFlow sets
    /// this to the line style's height so mixed-font runs stay baseline-aligned.
    #[rust]
    pub align_row_height: Option<f32>,

    /// The line's baseline below the row top (in unscaled lpxs) for RowAlign::Baseline;
    /// TextFlow sets it beside `align_row_height`, so every run on a row reports one line.
    #[rust]
    pub align_row_baseline: Option<f32>,

    /// The enclosing wrap flow's spacing when this text draws as one of its runs (see
    /// `LayoutOptions::flow_wrap_spacing_in_lpxs`); TextFlow sets it before each run.
    #[rust]
    pub flow_wrap_spacing_in_lpxs: Option<f32>,

    /// Per-row horizontal alignment applied by the text layouter when the
    /// text does not fill the full `max_width_in_lpxs`. `x: 0.0` = left,
    /// `0.5` = center, `1.0` = right. `y` is currently unused by the
    /// layouter. Reset to `Align::default()` per draw by callers that care.
    #[rust]
    pub layout_align: Align,

    /// Maximum number of lines to display. 0 means unlimited (default).
    /// When text exceeds this many lines, excess lines are hidden.
    /// Combined with `text_overflow: Ellipsis`, an ellipsis is appended
    /// to the last visible line.
    #[live(0usize)]
    pub max_lines: usize,

    /// Controls how text overflow is handled.
    /// `Clip` (default) clips text at the boundary.
    /// `Ellipsis` appends "…" at the truncation point.
    #[live]
    pub text_overflow: TextOverflow,

    /// When true, successive draws extend the area instead of replacing it.
    /// Useful when drawing multiple text chunks that should be treated as one area.
    #[live]
    pub extend_area: bool,

    // Align the instance payload, not just the whole struct. Otherwise
    // DrawText can have trailing padding that derived shaders read as their
    // first instance fields (for example a dropdown's focus and hover).
    // Keep all alignment padding before DrawVars and its contiguous payload.
    #[rust]
    draw_vars_align: DrawTextInstanceAlign,
    #[rust]
    draw_vars_padding: [u8; (16 - std::mem::size_of::<DrawVars>() % 16) % 16],

    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub rect_pos: Vec2f,
    #[live]
    pub rect_size: Vec2f,
    #[live]
    pub draw_clip: Vec4f,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live]
    pub glyph_depth: f32,
    #[live]
    pub texture_index: f32,
    #[live]
    pub char_index: f32,
    #[live(vec4(1., 1., 1., 1.))]
    pub color: Vec4f,
    #[live(1.0)]
    pub sdf_sharpness: f32,
    #[live(0.03)]
    pub sdf_luma_bias: f32,
    #[live]
    pub t_min: Vec2f,
    #[live]
    pub t_max: Vec2f,
    #[live]
    pub atlas_plane: f32,
    #[live]
    pub pad1: f32,
    #[live(0.0)]
    pub aa_2x2: f32,
    #[live(0.0)]
    pub aa_4x4: f32,
    #[live(0.2)]
    pub stem_darken: f32,
    #[live(0.125)]
    pub stem_darken_max: f32,
}

/// A glyph as it draws: slug curves (outline text) or an atlas image (colour/bitmap glyphs,
/// and outline text when the SDF/MSDF atlas is selected for its size).
#[derive(Clone, Copy, Debug)]
pub enum PreparedGlyph {
    Raster(RasterizedGlyph),
    Slug(crate::text::slug_atlas::SlugGlyphInfo),
}

impl PreparedGlyph {
    /// Still on its way from the host (a placeholder that covers nothing yet).
    pub fn is_pending(&self) -> bool {
        match self {
            PreparedGlyph::Raster(r) => r.atlas_kind == AtlasKind::Unresolved,
            PreparedGlyph::Slug(s) => s.curve_count == 0,
        }
    }

    /// The ink box's vertical centre, in ems above the baseline (negative: above).
    pub fn center_y_in_ems(&self) -> f32 {
        match self {
            PreparedGlyph::Raster(r) => (-r.origin_in_dpxs.y - r.atlas_image_bounds.size.height as f32 * 0.5) / r.dpxs_per_em,
            PreparedGlyph::Slug(s) => -s.origin_in_ems.y - s.size_in_ems.height * 0.5,
        }
    }
}

impl ScriptHook for DrawText {}

/// The glyphs looked up since the last call go to the draw list being recorded: while that list
/// is alive (painted again from its kept instances or not) they stay in use (`CxDrawList::glyph_uses`).
fn note_glyph_uses(cx: &mut Cx2d) {
    let Some(list) = cx.draw_list_stack.last().copied() else { return };
    let fonts = cx.fonts.clone();
    let fonts = fonts.borrow();
    let mut rasterizer = fonts.rasterizer().borrow_mut();
    rasterizer.take_glyph_uses(&mut cx.cx.draw_lists[list].glyph_uses);
}

/// How a laid-out glyph draws at `dpxs_per_em`: slug curves for outline glyphs (unless the
/// SDF/MSDF atlas is selected for this size), the atlas for colour/bitmap glyphs and outlines slug
/// cannot hold. A glyph without an outline or image (a space) is nothing: it never asks the host
/// for an atlas image. Also answers the slug generation a draw must upload (web), or 0.
fn prepare_glyph(cx: &mut Cx2d, glyph: &LaidoutGlyph, dpxs_per_em: f32) -> (Option<PreparedGlyph>, u64) {
    let raster_image = glyph.font.has_glyph_raster_image(glyph.id, dpxs_per_em);
    if !raster_image && cx.fonts.borrow().should_use_slug_glyph(dpxs_per_em) {
        let lookup = cx.fonts.borrow_mut().get_or_cache_slug_glyph(glyph.font.as_ref(), glyph.id);
        match lookup {
            SlugGlyphCacheResult::Ready(info) => return (Some(PreparedGlyph::Slug(info)), 0),
            SlugGlyphCacheResult::NeedsUpload { generation, glyph: info } => return (Some(PreparedGlyph::Slug(info)), generation),
            SlugGlyphCacheResult::Unavailable => {
                if glyph.font.glyph_outline_rc(glyph.id).is_none() {
                    return (None, 0);
                }
            }
        }
    }
    (glyph.rasterize(dpxs_per_em).map(PreparedGlyph::Raster), 0)
}

/// `G`: [`PreparedGlyph`] (`prepare_single_line_run`, drawn by `draw_prepared_glyph_abs`), or
/// [`RasterizedGlyph`] (`prepare_single_line_raster_run`, for shaders that sample the atlas).
#[derive(Clone, Debug)]
pub struct PreparedTextGlyph<G = PreparedGlyph> {
    pub pen_x_in_lpxs: f32,
    pub offset_x_in_lpxs: f32,
    pub advance_in_lpxs: f32,
    pub font_size_in_lpxs: f32,
    pub glyph: G,
}

#[derive(Clone, Debug)]
pub struct PreparedTextRun<G = PreparedGlyph> {
    pub width_in_lpxs: f32,
    pub ascender_in_lpxs: f32,
    pub descender_in_lpxs: f32,
    pub glyphs: Vec<PreparedTextGlyph<G>>,
}

impl DrawText {
    pub fn draw_abs(&mut self, cx: &mut Cx2d, pos: Vec2d, text: &str) {
        let text = self.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
        self.draw_text(cx, Point::new(pos.x as f32, pos.y as f32), &text);
    }

    pub fn begin_many_instances(&mut self, cx: &mut Cx2d) {
        if self.many_instances.is_some() {
            return;
        }
        self.update_draw_vars(cx);
        self.many_instances = cx.begin_many_aligned_instances(&self.draw_vars);
    }

    pub fn end_many_instances(&mut self, cx: &mut Cx2d) {
        if let Some(instances) = self.many_instances.take() {
            self.flush_slug_textures_if_allowed(cx);
            self.finish_many_instances(cx, instances);
        }
    }

    /// Defers SLUG atlas uploads while preserving the original draw-call order.
    ///
    /// This is useful for composite text widgets like `TextFlow`, which issue
    /// many separate `DrawText` runs interleaved with non-text draw calls. Those
    /// widgets need the glyph uploads to be coalesced, but cannot safely hold a
    /// single `many_instances` batch open across the whole widget.
    pub fn begin_deferred_slug_flush(&mut self) {
        self.slug_flush_defer_depth = self.slug_flush_defer_depth.saturating_add(1);
    }

    pub fn end_deferred_slug_flush(&mut self, cx: &mut Cx2d) {
        if self.slug_flush_defer_depth == 0 {
            return;
        }
        self.slug_flush_defer_depth -= 1;
        self.flush_slug_textures_if_allowed(cx);
    }

    pub fn draw_rasterized_glyphs_abs(
        &mut self,
        cx: &mut Cx2d,
        glyphs: &[(Point<f32>, f32, RasterizedGlyph)],
        color: Vec4f,
    ) {
        self.draw_rasterized_glyphs_abs_with_storage(cx, glyphs, color, None);
    }

    pub fn draw_rasterized_glyphs_abs_with_storage(
        &mut self,
        cx: &mut Cx2d,
        glyphs: &[(Point<f32>, f32, RasterizedGlyph)],
        color: Vec4f,
        storage: Option<&mut RecordingBuffer>,
    ) {
        if glyphs.is_empty() {
            return;
        }
        // Kept glyphs (prepared runs, cached cells) are snapshots: under the HAL font service a
        // glyph may have been on its way when it was prepared, upgraded since, or taken back.
        // Each draw looks them up again, as laid-out text does, which also keeps them in use.
        let rasterizer = cx.fonts.borrow().rasterizer().clone();
        // An already-open batch ran update_draw_vars when it began; running it
        // again per call dominates CPU when thousands of glyphs share a batch.
        if let Some(mut instances) = self.many_instances.take() {
            Self::reserve_glyph_storage(&mut instances.instances,
                glyphs.len() * self.draw_vars.as_slice().len(), storage);
            self.glyph_depth = self.draw_depth;
            self.color = color;
            for (origin_in_lpxs, font_size_in_lpxs, rasterized_glyph) in glyphs {
                self.draw_rasterized_glyph(
                    *origin_in_lpxs,
                    *font_size_in_lpxs,
                    None,
                    rasterizer.borrow_mut().refresh_glyph(*rasterized_glyph),
                    &mut instances.instances,
                );
            }
            self.many_instances = Some(instances);
            note_glyph_uses(cx);
            return;
        }

        self.update_draw_vars(cx);
        let Some(mut instances) = cx.begin_many_aligned_instances(&self.draw_vars) else {
            return;
        };

        Self::reserve_glyph_storage(&mut instances.instances,
            glyphs.len() * self.draw_vars.as_slice().len(), storage);

        self.glyph_depth = self.draw_depth;
        self.char_index = 0.0;
        self.color = color;
        for (origin_in_lpxs, font_size_in_lpxs, rasterized_glyph) in glyphs {
            self.draw_rasterized_glyph(
                *origin_in_lpxs,
                *font_size_in_lpxs,
                None,
                rasterizer.borrow_mut().refresh_glyph(*rasterized_glyph),
                &mut instances.instances,
            );
        }

        self.finish_many_instances(cx, instances);
        note_glyph_uses(cx);
    }

    fn reserve_glyph_storage(
        instances: &mut RecordingBuffer,
        additional: usize,
        storage: Option<&mut RecordingBuffer>,
    ) {
        if instances.is_empty() {
            if let Some(storage) = storage.filter(|storage| storage.capacity() >= additional) {
                if instances.swap_storage(storage) { instances.clear(); }
            }
        }
        instances.reserve(additional);
    }

    /// Draws a glyph from a prepared run (kept by the caller) at `origin_in_lpxs` (the pen on
    /// the baseline). Inside `begin_many_instances` it joins the open batch.
    pub fn draw_prepared_glyph_abs(
        &mut self,
        cx: &mut Cx2d,
        origin_in_lpxs: Point<f32>,
        font_size_in_lpxs: f32,
        glyph: PreparedGlyph,
        color: Vec4f,
    ) {
        let slug = match glyph {
            PreparedGlyph::Raster(raster) => {
                return self.draw_rasterized_glyph_abs(cx, origin_in_lpxs, font_size_in_lpxs, raster, color);
            }
            // a kept glyph is looked up again (curves upgraded, taken back, or still on their
            // way when it was prepared), which also keeps it in use
            PreparedGlyph::Slug(slug) => cx.fonts.borrow().rasterizer().borrow_mut().refresh_slug_glyph(slug),
        };
        let open = self.many_instances.take();
        let batch_was_open = open.is_some();
        let mut instances = match open {
            Some(instances) => instances,
            None => {
                self.update_draw_vars(cx);
                let Some(instances) = cx.begin_many_aligned_instances(&self.draw_vars) else { return };
                self.glyph_depth = self.draw_depth;
                self.char_index = 0.0;
                instances
            }
        };
        self.color = color;
        self.draw_slug_glyph(cx, origin_in_lpxs, font_size_in_lpxs, None, slug, &mut instances.instances);
        if batch_was_open {
            self.many_instances = Some(instances);
        } else {
            self.finish_many_instances(cx, instances);
        }
        note_glyph_uses(cx);
    }

    pub fn draw_rasterized_glyph_abs(
        &mut self,
        cx: &mut Cx2d,
        origin_in_lpxs: Point<f32>,
        font_size_in_lpxs: f32,
        rasterized_glyph: RasterizedGlyph,
        color: Vec4f,
    ) {
        self.draw_rasterized_glyphs_abs(
            cx,
            &[(origin_in_lpxs, font_size_in_lpxs, rasterized_glyph)],
            color,
        );
    }

    /// A drawable first line of `text` (its glyphs as they draw: slug curves, or atlas images
    /// for colour glyphs), and its metrics.
    pub fn prepare_single_line_run(&self, cx: &mut Cx2d, text: &str) -> Option<PreparedTextRun> {
        self.prepare_single_line_with(cx, text, |cx, glyph, dpx_per_em| prepare_glyph(cx, glyph, dpx_per_em).0)
    }

    /// The first line of `text` as atlas images, for shaders that sample the glyph atlas
    /// (rotated, 3D and diagonal text).
    pub fn prepare_single_line_raster_run(&self, cx: &mut Cx2d, text: &str) -> Option<PreparedTextRun<RasterizedGlyph>> {
        self.prepare_single_line_with(cx, text, |_, glyph, dpx_per_em| glyph.rasterize(dpx_per_em))
    }

    fn prepare_single_line_with<G>(
        &self,
        cx: &mut Cx2d,
        text: &str,
        mut prepare: impl FnMut(&mut Cx2d, &LaidoutGlyph, f32) -> Option<G>,
    ) -> Option<PreparedTextRun<G>> {
        let laidout = self.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
        let row = laidout.rows.first()?;
        if row.glyphs.is_empty() {
            return None;
        }

        let dpx_factor = cx.current_dpi_factor() as f32;
        let mut glyphs = Vec::with_capacity(row.glyphs.len());
        for glyph in &row.glyphs {
            // font_scale must scale the drawn glyph size together with the pen
            // advances below, or scaled runs render letter-spaced/overlapped;
            // rasterize at the scaled size so scaled-up text stays crisp
            let dpx_per_em = glyph.font_size_in_lpxs * dpx_factor * self.font_scale;
            let Some(prepared) = prepare(cx, glyph, dpx_per_em) else {
                continue;
            };

            glyphs.push(PreparedTextGlyph {
                pen_x_in_lpxs: glyph.origin_in_lpxs.x * self.font_scale,
                offset_x_in_lpxs: glyph.offset_in_lpxs() * self.font_scale,
                advance_in_lpxs: glyph.advance_in_lpxs() * self.font_scale,
                font_size_in_lpxs: glyph.font_size_in_lpxs * self.font_scale,
                glyph: prepared,
            });
        }
        if glyphs.is_empty() {
            return None;
        }

        Some(PreparedTextRun {
            width_in_lpxs: row.width_in_lpxs * self.font_scale,
            ascender_in_lpxs: row.ascender_in_lpxs * self.font_scale,
            descender_in_lpxs: row.descender_in_lpxs * self.font_scale,
            glyphs,
        })
    }

    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk, align: Align, text: &str) -> Rect {
        self.draw_walk_keep(cx, walk, align, text).0
    }

    /// `draw_walk`, also handing back the laid-out text, for a widget that
    /// maps points to text and back (selection).
    pub fn draw_walk_keep(&mut self, cx: &mut Cx2d, walk: Walk, align: Align, text: &str) -> (Rect, Rc<LaidoutText>) {
        self.draw_walk_restyled(cx, walk, align, text, None)
    }

    /// `draw_walk_keep`, with a chance to restyle the laid-out text before it
    /// is drawn (a copy with some glyphs recoloured, say: a label's links).
    /// `restyle` returns `None` to draw the layout as it is.
    pub fn draw_walk_restyled(
        &mut self,
        cx: &mut Cx2d,
        walk: Walk,
        align: Align,
        text: &str,
        restyle: Option<&dyn Fn(&LaidoutText) -> Option<LaidoutText>>,
    ) -> (Rect, Rc<LaidoutText>) {
        let walk = cx.resolve_walk(walk, ResolveAt::BeforeBegin);
        let mut max_width_in_lpxs = self.max_layout_width_for_walk(cx, walk);
        if max_width_in_lpxs.is_none()
            && (self.text_overflow == TextOverflow::Ellipsis || self.max_lines > 0)
        {
            let ellipsis_width_in_lpxs = (self.text_overflow == TextOverflow::Ellipsis).then(|| {
                self.layout(cx, 0.0, 0.0, None, false, Align::default(), "…")
                    .size_in_lpxs
                    .width
            });
            max_width_in_lpxs =
                self.fit_layout_width_for_walk(cx, walk, ellipsis_width_in_lpxs);
        }
        let wrap = matches!(cx.turtle().layout().flow, Flow::Right { wrap: true, .. });
        let laid = self.layout(cx, 0.0, 0.0, max_width_in_lpxs, wrap, align, text);
        let laid = match restyle {
            Some(restyle) => restyle(&laid).map(Rc::new).unwrap_or(laid),
            None => laid,
        };
        let rect = self.draw_walk_laidout(cx, walk, &laid);
        (rect, laid)
    }

    fn max_layout_width_for_walk(&self, cx: &mut Cx2d, walk: Walk) -> Option<f32> {
        let width = cx
            .turtle()
            .max_width(walk)
            .or_else(|| {
                let turtle_rect = cx.turtle().inner_rect();
                (!turtle_rect.size.x.is_nan()).then_some(turtle_rect.size.x)
            })?;

        Some((width.max(0.0) as f32) / self.font_scale.max(0.0001))
    }

    fn fit_layout_width_for_walk(
        &self,
        cx: &Cx2d,
        walk: Walk,
        ellipsis_width_in_lpxs: Option<f32>,
    ) -> Option<f32> {
        if !walk.width.is_fit() {
            return None;
        }
        let resolved = cx
            .walk_max_width(walk)
            .or_else(|| cx.current_turtle_max_width())?;
        let padding = cx.turtle().padding();
        let available_in_lpxs = ((resolved - padding.left - padding.right).max(0.0) as f32)
            / self.font_scale.max(0.0001);
        // A bound too narrow for even the truncation ellipsis is no bound at
        // all. This lets an enclosing wrapped flow relocate or hide the run.
        if ellipsis_width_in_lpxs.is_some_and(|ellipsis| available_in_lpxs < ellipsis) {
            None
        } else {
            Some(available_in_lpxs)
        }
    }

    /// Record this draw for a UI check (only called while one is collecting).
    fn note_text_run(&self, cx: &mut Cx2d, origin_in_lpxs: Point<f32>, text: &LaidoutText) {
        let Some(row) = text.rows.first() else { return };
        let Some(glyph) = row.glyphs.first() else { return };
        let scale = self.font_scale;
        let rect = rect(
            origin_in_lpxs.x as f64,
            origin_in_lpxs.y as f64,
            (text.size_in_lpxs.width * scale) as f64,
            (text.size_in_lpxs.height * scale) as f64,
        );
        let run = crate::cx_2d::TextRun {
            owner: 0,
            rect,
            font_size: glyph.font_size_in_lpxs * scale,
            cap_height: row.cap_height_in_lpxs * scale,
            truncated: text.is_truncated,
            color: [self.color.x, self.color.y, self.color.z, self.color.w],
            area: Area::Empty,
            declared_empty: false,
        };
        if let Some(sink) = cx.text_runs.as_mut() {
            sink.runs.push(crate::cx_2d::TextRun { owner: sink.owner, ..run });
        }
    }

    pub fn draw_walk_laidout(
        &mut self,
        cx: &mut Cx2d,
        walk: Walk,
        laidout_text: &LaidoutText,
    ) -> Rect {
        use crate::text::geom::{Point, Size};
        use crate::turtle;

        let walk = cx.resolve_walk(walk, ResolveAt::BeforeBegin);
        let size_in_lpxs = laidout_text.size_in_lpxs * self.font_scale;
        let max_size_in_lpxs = Size::new(
            cx.turtle()
                .max_width(walk)
                .map_or(size_in_lpxs.width, |max_width| max_width as f32),
            cx.turtle()
                .max_height(walk)
                .map_or(size_in_lpxs.height, |max_height| max_height as f32),
        );
        // The ink shift moves the glyphs inside the box the walk takes, so the box,
        // and every layout decision made from it (this call's return value included),
        // stays exactly where it was.
        let ink_shift_in_lpxs = if self.ink_centered {
            laidout_text.ink_center_offset_in_lpxs() * self.font_scale
        } else {
            0.0
        };
        // The first row's baseline as drawn: its ascender, the ink shift and the
        // glyph shift (which scales the unscaled glyph size, as `draw_glyph` does).
        let shift_y_in_lpxs = laidout_text
            .rows
            .first()
            .and_then(|row| row.glyphs.first())
            .map_or(0.0, |glyph| glyph.font_size_in_lpxs * self.temp_y_shift);
        let measured_baseline = laidout_text.rows.first().map(|row| {
            row.origin_in_lpxs.y * self.font_scale + ink_shift_in_lpxs + shift_y_in_lpxs
        });
        let turtle_rect = cx.walk_turtle(Walk {
            width: turtle::Size::Fixed(max_size_in_lpxs.width as f64),
            height: turtle::Size::Fixed(max_size_in_lpxs.height as f64),
            metrics: Metrics {
                descender: -laidout_text.rows.last().unwrap().descender_in_lpxs as f64,
                line_gap: 0.0,
                line_scale: 1.0,
            },
            baseline: match walk.baseline {
                Baseline::Auto => measured_baseline.map_or(Baseline::Auto, Baseline::At),
                declared => declared,
            },
            ..walk
        });

        if self.debug {
            let mut area = Area::Empty;
            cx.add_aligned_rect_area(&mut area, turtle_rect);
            cx.cx.debug.area(area, vec4(1.0, 1.0, 1.0, 1.0));
        }

        let origin_in_lpxs = Point::new(turtle_rect.pos.x as f32, turtle_rect.pos.y as f32);
        let runs_before = cx.text_runs.as_ref().map(|sink| sink.runs.len());
        let declared_empty = turtle_rect.size.x <= 0.0 || turtle_rect.size.y <= 0.0;
        self.draw_text(
            cx,
            Point::new(origin_in_lpxs.x, origin_in_lpxs.y + ink_shift_in_lpxs),
            laidout_text,
        );
        if let Some(before) = runs_before {
            // The run just recorded is the walk's; its area is the draw's now.
            let area = self.draw_vars.area;
            if let Some(run) = cx.text_runs.as_mut().and_then(|sink| sink.runs.get_mut(before)) {
                run.area = area;
                run.declared_empty = declared_empty;
            }
        }

        rect(
            origin_in_lpxs.x as f64,
            origin_in_lpxs.y as f64,
            size_in_lpxs.width as f64,
            size_in_lpxs.height as f64,
        )
    }

    /// Draws text within the current turtle flow, calling `f` for each laid-out row.
    /// Returns `(row_count, is_truncated)`: the number of rows produced, and whether
    /// the text was truncated (e.g., by `max_lines` / ellipsis).
    ///
    /// When text wraps to multiple visual rows, each row is emitted as its own
    /// `FinishedWalk` with a single-row `outer_size.y`. Between rows,
    /// `turtle_new_line_with_spacing` is called to finish the previous visual
    /// row (triggering `finish_row` → `RowAlign::Center`, etc.) and position
    /// the turtle for the next row. This lets per-row alignment work correctly
    /// when inline block widgets (pills, badges, etc.) share a visual row with
    /// text of a different height.
    pub fn draw_walk_resumable_with(
        &mut self,
        cx: &mut Cx2d,
        text_str: &str,
        mut f: impl FnMut(&mut Cx2d, Rect, f32),
    ) -> (usize, bool) {
        self.draw_walk_resumable_with_inner(cx, text_str, false, &mut f)
    }

    /// Like [`Self::draw_walk_resumable_with`], but invokes the callback before
    /// emitting glyphs. Use this for backgrounds that must sit behind text.
    pub fn draw_walk_resumable_with_background(
        &mut self,
        cx: &mut Cx2d,
        text_str: &str,
        mut f: impl FnMut(&mut Cx2d, Rect, f32),
    ) -> (usize, bool) {
        self.draw_walk_resumable_with_inner(cx, text_str, true, &mut f)
    }

    fn draw_walk_resumable_with_inner(
        &mut self,
        cx: &mut Cx2d,
        text_str: &str,
        callback_before_text: bool,
        f: &mut impl FnMut(&mut Cx2d, Rect, f32),
    ) -> (usize, bool) {
        // ── Layout parameters ──
        let turtle_pos = cx.turtle().pos();
        let turtle_rect = cx.turtle().inner_rect();
        let origin_in_lpxs = Point::new(turtle_rect.pos.x as f32, turtle_pos.y as f32);
        let first_row_indent_in_lpxs = turtle_pos.x as f32 - origin_in_lpxs.x;
        let row_height = self.resumable_first_row_min_spacing(cx) as f64;
        let first_row_drop = self.first_row_anchor_drop(cx);
        let max_width_in_lpxs = if !turtle_rect.size.x.is_nan() {
            Some(turtle_rect.size.x as f32)
        } else {
            None
        };
        let wrap = matches!(cx.turtle().layout().flow, Flow::Right { wrap: true, .. });

        // ── Text layout ──
        let text = self.layout(
            cx,
            first_row_indent_in_lpxs,
            row_height as f32,
            max_width_in_lpxs,
            wrap,
            self.layout_align,
            text_str,
        );

        // ── Common computations ──
        let last_row = text.rows.last().unwrap();
        let new_turtle_pos = {
            let p = origin_in_lpxs
                + Size::new(
                    last_row.width_in_lpxs,
                    last_row.origin_in_lpxs.y - last_row.ascender_in_lpxs,
                ) * self.font_scale;
            dvec2(p.x as f64, p.y as f64)
        };
        let used_size_in_lpxs = text.size_in_lpxs * self.font_scale;
        let shift_y = text
            .rows
            .first()
            .and_then(|r| r.glyphs.first())
            .map(|g| g.font_size_in_lpxs * self.temp_y_shift)
            .unwrap_or(0.0);
        // A run reports its line's baseline, not its own glyphs', so scaled and shifted
        // runs keep their offsets from the line; a run outside a flow reports its own.
        let (align_row_baseline, font_scale) = (self.align_row_baseline, self.font_scale);
        let line_baseline = move |row: &LaidoutRow| -> f64 {
            align_row_baseline.map_or(
                (row.ascender_in_lpxs * font_scale + shift_y) as f64,
                |baseline| (baseline * font_scale) as f64,
            )
        };

        // ── Decide whether to use per-row glyph batching ──
        // Per-row batching gives each visual row its own AlignEntry so that
        // finish_row alignment shifts apply independently per row. This
        // requires a fresh draw (no `many_instances` reuse buffer).
        let per_row = !self.draws_wrapped_run_as_single_walk() && text.rows.len() > 1;


        if per_row {
            // ── PER-ROW path ──
            // Draw each visual row as a separate instance batch so its glyphs
            // get their own AlignEntry. Emit a FinishedWalk per row and call
            // turtle_new_line between rows so finish_row runs at each row
            // boundary — enabling RowAlign::Center to center text relative to
            // any inline widget (pill) on the same visual row.

            self.update_draw_vars(cx);
            self.glyph_depth = self.draw_depth;
            self.char_index = 0.0;
            let saved_extend_area = self.extend_area;

            for (row_idx, row) in text.rows.iter().enumerate() {
                let row_h =
                    ((row.ascender_in_lpxs - row.descender_in_lpxs) * self.font_scale) as f64;

                // Between rows: finish the previous visual row (triggers
                // finish_row → RowAlign::Center) and position for the next.
                if row_idx > 0 {
                    let ws = cx.turtle().wrap_spacing();
                    cx.turtle_new_line_with_spacing(ws);
                    self.extend_area = true;
                }

                // Compute row_origin: for row 0 (continuation row), use the
                // layouter's position (correct indent on the existing line).
                // For wrapped rows (1+), use the TURTLE's current position so
                // glyphs land where the turtle tracks — not at the layouter's
                // position which doesn't account for taller inline widgets
                // (pills) inflating the previous row's height.
                let row_origin = if row_idx == 0 {
                    origin_in_lpxs + Size::from(row.origin_in_lpxs) * self.font_scale
                } else {
                    let tp = cx.turtle().pos();
                    Point::new(
                        tp.x as f32 + row.origin_in_lpxs.x * self.font_scale,
                        tp.y as f32 + row.ascender_in_lpxs * self.font_scale,
                    )
                };
                let row_top_y = (row_origin.y - row.ascender_in_lpxs * self.font_scale) as f64;

                let row_als = cx.align_list_len();
                let (sx, ex) =
                    row_span_x_bounds_in_lpxs(row, row_idx == 0, row_idx + 1 == text.rows.len());
                let row_rect = rect(
                    (row_origin.x + sx * self.font_scale) as f64,
                    row_top_y + shift_y as f64,
                    ((ex - sx) * self.font_scale) as f64,
                    row_h,
                );

                if callback_before_text {
                    f(cx, row_rect, row.ascender_in_lpxs);
                }

                // Draw this row's glyphs as a separate aligned-instance batch.
                if let Some(mut instances) = cx.begin_many_aligned_instances(&self.draw_vars) {
                    self.draw_row(cx, row_origin, row, &mut instances.instances);
                    self.finish_many_instances(cx, instances);
                }

                // Update turtle allocation for this row.
                if row_idx == 0 {
                    let turtle = cx.turtle_mut();
                    turtle.move_to(dvec2(origin_in_lpxs.x as f64, origin_in_lpxs.y as f64));
                    turtle.allocate_width(used_size_in_lpxs.width as f64);
                    turtle.allocate_height(row_h);
                } else {
                    let turtle = cx.turtle_mut();
                    turtle.allocate_height(row_h);
                    turtle.allocate_width((row.width_in_lpxs * self.font_scale) as f64);
                }

                // Set wrap spacing from this row's line-spacing metrics.
                cx.turtle_mut().set_wrap_spacing(
                    (row.ascender_in_lpxs * row.line_spacing_scale - row.ascender_in_lpxs) as f64,
                );

                // Call the `f` callback for this row (draws strikethrough,
                // underline, or plain area tracking).
                if !callback_before_text {
                    f(cx, row_rect, row.ascender_in_lpxs);
                }

                // Emit a per-row FinishedWalk covering this row's glyphs +
                // callback rect-areas.
                cx.emit_turtle_walk_with_role(
                    Rect {
                        pos: dvec2(row_origin.x as f64, row_top_y),
                        size: dvec2((row.width_in_lpxs * self.font_scale) as f64, row_h),
                    },
                    row_als,
                    Metrics::default(),
                    self.align_row_height
                        .map(|h| (h * self.font_scale) as f64),
                    Some(line_baseline(row)),
                    crate::turtle::RowAlignRole::Shiftable,
                );
            }

            // Position turtle at end of last row for subsequent inline content.
            // pos.y is already at the last row's top (from turtle_new_line).
            // Advance pos.x past the last row's text width.
            let last = text.rows.last().unwrap();
            let end_x = cx.turtle().pos().x + (last.width_in_lpxs * self.font_scale) as f64;
            let end_y = cx.turtle().pos().y;
            cx.turtle_mut().move_to(dvec2(end_x, end_y));
            self.extend_area = saved_extend_area;
            self.flush_slug_textures_if_allowed(cx);
        } else {
            // ── SINGLE-WALK path (single-row text, reuse mode, or Linux/Win) ──
            // A wrapped run's rows share one batch, so the whole run drops to center its first
            // row on taller content before it. The layout already pulled the later rows up by it.
            let run_drop = if text.rows.len() > 1 { first_row_drop } else { 0.0 };
            let row_top_in_lpxs = origin_in_lpxs.y as f64;
            let origin_in_lpxs = Point::new(origin_in_lpxs.x, origin_in_lpxs.y + run_drop as f32);
            let new_turtle_pos = new_turtle_pos + dvec2(0.0, run_drop);
            let align_list_start = cx.align_list_len();
            if callback_before_text {
                for (row_index, row) in text.rows.iter().enumerate() {
                    let (sx, ex) = row_span_x_bounds_in_lpxs(
                        row,
                        row_index == 0,
                        row_index + 1 == text.rows.len(),
                    );
                    f(
                        cx,
                        rect(
                            (origin_in_lpxs.x + (row.origin_in_lpxs.x + sx) * self.font_scale)
                                as f64,
                            (origin_in_lpxs.y
                                + (row.origin_in_lpxs.y - row.ascender_in_lpxs) * self.font_scale)
                                as f64
                                + shift_y as f64,
                            ((ex - sx) * self.font_scale) as f64,
                            ((row.ascender_in_lpxs - row.descender_in_lpxs) * self.font_scale)
                                as f64,
                        ),
                        row.ascender_in_lpxs,
                    );
                }
            }

            self.draw_text(cx, origin_in_lpxs, &text);

            if !callback_before_text {
                for (row_index, row) in text.rows.iter().enumerate() {
                    let (sx, ex) = row_span_x_bounds_in_lpxs(
                        row,
                        row_index == 0,
                        row_index + 1 == text.rows.len(),
                    );
                    f(
                        cx,
                        rect(
                            (origin_in_lpxs.x + (row.origin_in_lpxs.x + sx) * self.font_scale)
                                as f64,
                            (origin_in_lpxs.y
                                + (row.origin_in_lpxs.y - row.ascender_in_lpxs) * self.font_scale)
                                as f64
                                + shift_y as f64,
                            ((ex - sx) * self.font_scale) as f64,
                            ((row.ascender_in_lpxs - row.descender_in_lpxs) * self.font_scale)
                                as f64,
                        ),
                        row.ascender_in_lpxs,
                    );
                }
            }

            let wrap_spacing = (last_row.ascender_in_lpxs * last_row.line_spacing_scale
                - last_row.ascender_in_lpxs) as f64;

            if text.rows.len() == 1 {
                let turtle = cx.turtle_mut();
                turtle.move_to(dvec2(origin_in_lpxs.x as f64, origin_in_lpxs.y as f64));
                turtle.allocate_width(used_size_in_lpxs.width as f64);
                turtle.allocate_height(used_size_in_lpxs.height as f64);
                turtle.move_to(new_turtle_pos);
                turtle.set_wrap_spacing(wrap_spacing);

                cx.emit_turtle_walk_with_role(
                    Rect {
                        pos: new_turtle_pos,
                        size: dvec2(
                            used_size_in_lpxs.width as f64,
                            used_size_in_lpxs.height as f64,
                        ),
                    },
                    align_list_start,
                    Metrics::default(),
                    self.align_row_height.map(|h| (h * self.font_scale) as f64),
                    Some(line_baseline(last_row)),
                    crate::turtle::RowAlignRole::Shiftable,
                );
            } else {
                // The turtle's row bookkeeping must see this run's internal
                // row boundaries: RowAlign shifts and row heights are computed
                // per turtle row when `finish_row` runs, and without a boundary
                // here every walk since the previous one — including content
                // that sits on an earlier visual row — is aligned against the
                // final row's height. Only the finished-walk and row
                // bookkeeping is per-row; the glyphs stay in one instance
                // batch, so no per-row shifting of this run's own rows is
                // possible and its walks carry ranges accordingly.
                let fs = self.font_scale as f64;
                let row_top = |row: &crate::text::layouter::LaidoutRow| {
                    origin_in_lpxs.y as f64
                        + (row.origin_in_lpxs.y - row.ascender_in_lpxs) as f64 * fs
                };
                let row_height = |row: &crate::text::layouter::LaidoutRow| {
                    (row.ascender_in_lpxs - row.descender_in_lpxs) as f64 * fs
                };


                let first_row = &text.rows[0];
                let turtle = cx.turtle_mut();
                turtle.move_to(dvec2(origin_in_lpxs.x as f64, row_top_in_lpxs));
                turtle.allocate_width(used_size_in_lpxs.width as f64);
                turtle.allocate_height(run_drop + row_height(first_row));

                // The first row's walk owns every align entry this run emitted
                // and is immovable — shifting it would displace the later
                // rows' glyphs, which share its instance batch. When the first
                // row holds visible text it anchors its row's center line, so
                // a taller inline item beside it centers on the text; when it
                // holds nothing visible — no glyphs, or only whitespace (a
                // continuation that wrapped immediately, often with a single
                // leading space fitting the line's slack) — the walk is inert,
                // so the row of preceding content keeps its normal alignment
                // instead of anchoring to an invisible line.
                let first_row_role = if first_row.text.trim().is_empty() {
                    crate::turtle::RowAlignRole::Fixed
                } else {
                    crate::turtle::RowAlignRole::Anchor
                };
                cx.emit_turtle_walk_with_role(
                    Rect {
                        pos: dvec2(origin_in_lpxs.x as f64, row_top_in_lpxs),
                        size: dvec2(
                            (first_row.width_in_lpxs * self.font_scale) as f64,
                            run_drop + row_height(first_row),
                        ),
                    },
                    align_list_start,
                    Metrics::default(),
                    // Padding the centering height by the drop on both sides keeps the row's
                    // center on the dropped text's center.
                    self.align_row_height
                        .map(|h| (h * self.font_scale) as f64 + 2.0 * run_drop),
                    Some(run_drop + line_baseline(first_row)),
                    first_row_role,
                );

                for row in &text.rows[1..] {
                    // Finishing a row can forgive part of its height, so rather than trusting
                    // the advance we put the turtle right on this row's glyphs, which can't move.
                    cx.turtle_new_line();
                    let x = cx.turtle().pos().x;
                    cx.turtle_mut().move_to(dvec2(x, row_top(row)));
                    cx.turtle_mut().allocate_height(row_height(row));
                }

                let turtle = cx.turtle_mut();
                turtle.move_to(new_turtle_pos);
                turtle.set_wrap_spacing(wrap_spacing);

                // The last row's walk carries an empty align range: this run's
                // glyphs live in the first row's walk and must not be shifted
                // again when the last row finishes. Because those glyphs can
                // never move, the walk anchors the row's center line — a
                // taller item that joins this row (an inline pill) centers on
                // the text instead of dragging the row's center below it.
                let empty_range = cx.align_list_len();
                cx.emit_turtle_walk_with_role(
                    Rect {
                        pos: new_turtle_pos,
                        size: dvec2(
                            (last_row.width_in_lpxs * self.font_scale) as f64,
                            row_height(last_row),
                        ),
                    },
                    empty_range,
                    Metrics::default(),
                    // Center on the line's height like the first row does, so a scaled
                    // run (code, sub/superscript) doesn't drag the row's center line.
                    self.align_row_height
                        .map(|h| (h * self.font_scale) as f64)
                        .or(Some(row_height(last_row))),
                    Some(line_baseline(last_row)),
                    crate::turtle::RowAlignRole::Anchor,
                );
            }
        }

        (text.rows.len(), text.is_truncated)
    }

    /// The floor a resumable run passes to the layouter under Center or Baseline wrapping: how
    /// far the current row reaches below the run's first-row top (`font_scale` divided out), so
    /// its second row clears taller inline content. Every layout of a run must pass this value.
    pub fn resumable_first_row_min_spacing(&self, cx: &Cx2d) -> f32 {
        match cx.turtle().layout().flow {
            Flow::Right {
                wrap: true,
                row_align: crate::turtle::RowAlign::Center,
                ..
            } => {
                // The per-row path finishes this run's first row like any other: the row is as
                // tall as its tallest item and the text centers on it, so the floor is the row's
                // box measured from the centered text's top; the layouter adds the wrap gap.
                let row_height = cx.turtle().row_height();
                let text_row_height = self
                    .align_row_height
                    .map(|h| (h * self.font_scale) as f64)
                    .unwrap_or(row_height);
                let centering = ((row_height - text_row_height) * 0.5).max(0.0);
                ((row_height - centering) / self.font_scale.max(0.0001) as f64) as f32
            }
            Flow::Right {
                wrap: true,
                row_align: crate::turtle::RowAlign::Baseline,
                ..
            } => {
                // The per-row path seats this run's first row on the row's line, so the next
                // row must clear whatever hangs deepest below that line.
                let Some(baseline) = self.align_row_baseline else {
                    return 0.0;
                };
                let fs = self.font_scale.max(0.0001) as f64;
                ((baseline as f64 * fs + cx.row_baseline_descent()) / fs) as f32
            }
            _ => 0.0,
        }
    }

    /// Whether a wrapped run draws as one immovable batch. The per-row path (macOS, iOS,
    /// Android, web) aligns each row on its own and needs a fresh draw.
    fn draws_wrapped_run_as_single_walk(&self) -> bool {
        self.many_instances.is_some()
    }

    /// How far a single-walk run's first row must sit below the turtle row's top so that
    /// taller content before it on the row can align to it (see `Cx2d::row_anchor_drop`).
    fn first_row_anchor_drop(&self, cx: &Cx2d) -> f64 {
        if !self.draws_wrapped_run_as_single_walk() {
            return 0.0;
        }
        self.align_row_height.map_or(0.0, |height| {
            cx.row_anchor_drop(
                (height * self.font_scale) as f64,
                self.align_row_baseline.map(|baseline| (baseline * self.font_scale) as f64),
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn layout(
        &self,
        cx: &mut Cx,
        first_row_indent_in_lpxs: f32,
        first_row_min_line_spacing_below_in_lpxs: f32,
        max_width_in_lpxs: Option<f32>,
        wrap: bool,
        align: Align,
        text: &str,
    ) -> Rc<LaidoutText> {
        self.text_style.font_family.ensure_fonts_loaded(cx);
        let laidout = self.layout_loaded(
            cx,
            first_row_indent_in_lpxs,
            first_row_min_line_spacing_below_in_lpxs,
            max_width_in_lpxs,
            wrap,
            align,
            text,
        );
        let has_missing_glyph = laidout
            .rows
            .iter()
            .flat_map(|row| &row.glyphs)
            .any(|glyph| glyph.id == 0);
        // A glyph the family's own faces miss brings in its lazy CJK or emoji
        // face (read from disk here, a memory map); laid out again at once,
        // the text shows the real glyph on this very frame, not a box first.
        if !has_missing_glyph {
            return laidout;
        }
        let started = Cx::time_now();
        if self.text_style.font_family.request_lazy_fonts_for_glyph_miss(
            cx,
            text,
            has_missing_glyph,
        ) {
            let laidout = self.layout_loaded(
                cx,
                first_row_indent_in_lpxs,
                first_row_min_line_spacing_below_in_lpxs,
                max_width_in_lpxs,
                wrap,
                align,
                text,
            );
            log!(
                "lazy font face loaded and text laid out again in {:.2} ms",
                (Cx::time_now() - started) * 1000.0
            );
            return laidout;
        }
        laidout
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_loaded(
        &self,
        cx: &mut Cx,
        first_row_indent_in_lpxs: f32,
        first_row_min_line_spacing_below_in_lpxs: f32,
        max_width_in_lpxs: Option<f32>,
        wrap: bool,
        align: Align,
        text: &str,
    ) -> Rc<LaidoutText> {
        let fonts = cx.get_global::<Rc<RefCell<Fonts>>>().clone();
        let mut fonts = fonts.borrow_mut();

        let laidout = fonts.get_or_layout(BorrowedLayoutParams {
            text,
            style: Style {
                font_family_id: self.text_style.font_family.to_font_family_id(),
                font_size_in_pts: self.text_style.font_size,
                color: None,
            },
            options: LayoutOptions {
                first_row_indent_in_lpxs,
                first_row_min_line_spacing_below_in_lpxs,
                max_width_in_lpxs,
                wrap,
                align: align.x as f32,
                line_spacing_scale: self.text_style.line_spacing,
                flow_wrap_spacing_in_lpxs: self.flow_wrap_spacing_in_lpxs,
                max_rows: if self.max_lines > 0 {
                    Some(self.max_lines)
                } else {
                    None
                },
                ellipsis: self.text_overflow == TextOverflow::Ellipsis,
            },
        });
        drop(fonts);
        laidout
    }

    fn draw_text(&mut self, cx: &mut Cx2d, origin_in_lpxs: Point<f32>, text: &LaidoutText) {
        if cx.text_runs.is_some() {
            self.note_text_run(cx, origin_in_lpxs, text);
        }
        {
            self.update_draw_vars(cx);
            if let Some(mut instances) = self.many_instances.take() {
                self.glyph_depth = self.draw_depth;
                for row in &text.rows {
                    self.draw_row(
                        cx,
                        origin_in_lpxs + Size::from(row.origin_in_lpxs) * self.font_scale,
                        row,
                        &mut instances.instances,
                    );
                }
                self.many_instances = Some(instances);
                note_glyph_uses(cx);
                return;
            }
            let Some(mut instances) = cx.begin_many_aligned_instances(&self.draw_vars) else {
                return;
            };
            self.glyph_depth = self.draw_depth;
            self.char_index = 0.0;
            for row in &text.rows {
                self.draw_row(
                    cx,
                    origin_in_lpxs + Size::from(row.origin_in_lpxs) * self.font_scale,
                    row,
                    &mut instances.instances,
                );
            }
            self.flush_slug_textures_if_allowed(cx);
            self.finish_many_instances(cx, instances);
            note_glyph_uses(cx);
        }
    }

    fn flush_slug_textures_if_allowed(&mut self, cx: &mut Cx2d) {
        if self.slug_flush_defer_depth != 0 {
            return;
        }
        self.flush_slug_textures_if_needed(cx);
    }

    fn flush_slug_textures_if_needed(&mut self, cx: &mut Cx2d) {
        if self.pending_slug_flush_generation == 0 {
            return;
        }
        let fonts = cx.fonts.clone();
        let mut fonts = fonts.borrow_mut();
        if fonts.slug_uploaded_generation() < self.pending_slug_flush_generation {
            fonts.flush_slug_textures(cx.cx);
        }
        if fonts.slug_uploaded_generation() >= self.pending_slug_flush_generation {
            self.pending_slug_flush_generation = 0;
        }
    }

    fn finish_many_instances(&mut self, cx: &mut Cx2d, instances: ManyInstances) {
        let new_area = cx.end_many_instances(instances);
        let old_area = self.draw_vars.area;
        if self.extend_area {
            let extended = old_area.extend_with(cx, new_area);
            self.draw_vars.area = cx.update_area_refs(old_area, extended);
        } else {
            self.draw_vars.area = cx.update_area_refs(old_area, new_area);
        }
    }

    pub(crate) fn update_draw_vars(&mut self, cx: &mut Cx2d) {
        self.draw_vars.append_group_id = cx.draw_call_group_content().0;
        let fonts = cx.fonts.borrow();
        let rasterizer = fonts.rasterizer().borrow();
        let sdfer_settings = rasterizer.sdfer().settings();
        self.draw_vars.fit_dyn_uniforms(2);
        self.draw_vars.dyn_uniforms[0] = sdfer_settings.radius;
        self.draw_vars.dyn_uniforms[1] = sdfer_settings.cutoff;
        self.draw_vars.texture_slots[0] = Some(fonts.grayscale_texture().clone());
        self.draw_vars.texture_slots[1] = Some(fonts.color_texture().clone());
        self.draw_vars.texture_slots[2] = Some(fonts.msdf_texture().clone());
        {
            self.draw_vars.texture_slots[3] = Some(fonts.slug_curve_texture().clone());
            self.draw_vars.texture_slots[4] = Some(fonts.slug_band_texture().clone());

            // The camera itself is read by the shader at render time
            // (`slug_dilate`); only the viewport is the draw's.
            let viewport = cx.current_pass_size();
            let dpi_factor = cx.current_dpi_factor() as f32;
            let viewport_px = [
                (viewport.x as f32 * dpi_factor).max(1.0),
                (viewport.y as f32 * dpi_factor).max(1.0),
            ];

            self.draw_vars
                .set_uniform(cx.cx, live_id!(slug_viewport_px), &viewport_px);
        }
    }

    fn draw_debug_row_guides(
        &mut self,
        cx: &mut Cx2d,
        origin_in_lpxs: Point<f32>,
        row: &LaidoutRow,
    ) {
        if !self.debug {
            return;
        }

        let width_in_lpxs = row.width_in_lpxs * self.font_scale;
        let mut area = Area::Empty;
        cx.add_aligned_rect_area(
            &mut area,
            rect(
                origin_in_lpxs.x as f64,
                (origin_in_lpxs.y - row.ascender_in_lpxs * self.font_scale) as f64,
                width_in_lpxs as f64,
                1.0,
            ),
        );
        cx.cx.debug.area(area, vec4(1.0, 0.0, 0.0, 1.0));
        let mut area = Area::Empty;
        cx.add_aligned_rect_area(
            &mut area,
            rect(
                origin_in_lpxs.x as f64,
                origin_in_lpxs.y as f64,
                width_in_lpxs as f64,
                1.0,
            ),
        );
        cx.cx.debug.area(area, vec4(0.0, 1.0, 0.0, 1.0));
        let mut area = Area::Empty;
        cx.add_aligned_rect_area(
            &mut area,
            rect(
                origin_in_lpxs.x as f64,
                (origin_in_lpxs.y - row.descender_in_lpxs * self.font_scale) as f64,
                width_in_lpxs as f64,
                1.0,
            ),
        );
        cx.cx.debug.area(area, vec4(0.0, 0.0, 1.0, 1.0));
    }

    fn draw_row(
        &mut self,
        cx: &mut Cx2d,
        origin_in_lpxs: Point<f32>,
        row: &LaidoutRow,
        out_instances: &mut RecordingBuffer,
    ) {
        for glyph in &row.glyphs {
            self.draw_glyph(
                cx,
                origin_in_lpxs + Size::from(glyph.origin_in_lpxs) * self.font_scale,
                glyph,
                out_instances,
            );
        }
        self.draw_debug_row_guides(cx, origin_in_lpxs, row);
    }

    fn resolve_glyph(&mut self, cx: &mut Cx2d, glyph: &LaidoutGlyph) -> Option<PreparedGlyph> {
        let font_size_in_dpxs = glyph.font_size_in_lpxs * cx.current_dpi_factor() as f32;
        let (prepared, upload_generation) = prepare_glyph(cx, glyph, font_size_in_dpxs);
        self.pending_slug_flush_generation = self.pending_slug_flush_generation.max(upload_generation);
        prepared
    }

    fn draw_glyph(
        &mut self,
        cx: &mut Cx2d,
        origin_in_lpxs: Point<f32>,
        glyph: &LaidoutGlyph,
        output: &mut RecordingBuffer,
    ) {
        use crate::text::geom::Point;
        let glyph_origin = Point::new(
            origin_in_lpxs.x + glyph.offset_in_lpxs() * self.font_scale,
            origin_in_lpxs.y,
        );

        match self.resolve_glyph(cx, glyph) {
            Some(PreparedGlyph::Slug(slug_glyph)) => {
                self.draw_slug_glyph(
                    cx,
                    glyph_origin,
                    glyph.font_size_in_lpxs,
                    glyph.color,
                    slug_glyph,
                    output,
                );
            }
            Some(PreparedGlyph::Raster(rasterized_glyph)) => self.draw_rasterized_glyph(
                glyph_origin,
                glyph.font_size_in_lpxs,
                glyph.color,
                rasterized_glyph,
                output,
            ),
            None => {}
        }
    }

    fn draw_slug_glyph(
        &mut self,
        cx: &mut Cx2d,
        origin_in_lpxs: Point<f32>,
        font_size_in_lpxs: f32,
        color: Option<Color>,
        glyph: crate::text::slug_atlas::SlugGlyphInfo,
        output: &mut RecordingBuffer,
    ) {
        let bounds_in_lpxs = TextRect::new(
            Point::new(
                origin_in_lpxs.x + glyph.origin_in_ems.x * font_size_in_lpxs * self.font_scale,
                origin_in_lpxs.y
                    + (-glyph.origin_in_ems.y - glyph.size_in_ems.height)
                        * font_size_in_lpxs
                        * self.font_scale,
            ),
            Size::new(
                glyph.size_in_ems.width * font_size_in_lpxs * self.font_scale,
                glyph.size_in_ems.height * font_size_in_lpxs * self.font_scale,
            ),
        );

        let pad = (self.get_aa_pad_px(cx.cx) / cx.current_dpi_factor() as f32).max(0.0);
        self.rect_pos = vec2(bounds_in_lpxs.origin.x - pad, bounds_in_lpxs.origin.y - pad)
            + vec2(0.0, self.temp_y_shift * font_size_in_lpxs);
        self.rect_size = vec2(
            bounds_in_lpxs.size.width + pad * 2.0,
            bounds_in_lpxs.size.height + pad * 2.0,
        );
        if let Some(color) = color {
            self.color = vec4(
                color.r as f32,
                color.g as f32,
                color.b as f32,
                color.a as f32,
            ) / 255.0;
        }
        self.texture_index = 3.0;
        self.atlas_plane = glyph.fill_flags as f32;
        self.t_min = vec2(glyph.curve_offset as f32, glyph.curve_count as f32);
        self.t_max = vec2(glyph.band_offset as f32, glyph.band_count as f32);
        let slice = self.draw_vars.as_slice();

        output.extend_from_slice(slice);
        self.glyph_depth += 0.000001;
        self.char_index += 1.0;
    }

    fn draw_rasterized_glyph(
        &mut self,
        origin_in_lpxs: Point<f32>,
        font_size_in_lpxs: f32,
        color: Option<Color>,
        glyph: RasterizedGlyph,
        output: &mut RecordingBuffer,
    ) {
        fn tex_coord(point: Point<usize>, size: Size<usize>) -> Point<f32> {
            Point::new(
                point.x as f32 / size.width as f32,
                point.y as f32 / size.height as f32,
            )
        }

        let texture_index = match glyph.atlas_kind {
            AtlasKind::Grayscale => 0.0,
            AtlasKind::Color => 1.0,
            AtlasKind::Msdf => 2.0,
            AtlasKind::Unresolved => -1.0,
        };

        let atlas_image_bounds = glyph.atlas_image_bounds;
        let atlas_size = glyph.atlas_size;
        let (t_min, t_max) = if glyph.atlas_kind == AtlasKind::Unresolved {
            // The image bounds below still describe the full glyph; the shader samples only
            // the reserved transparent texel, even before we know the host atlas dimensions.
            (Point::new(0.0, 0.0), Point::new(0.0, 0.0))
        } else {
            (
                tex_coord(atlas_image_bounds.min(), atlas_size),
                tex_coord(atlas_image_bounds.max(), atlas_size),
            )
        };

        let atlas_image_padding = glyph.atlas_image_padding;
        let atlas_image_size = atlas_image_bounds.size;
        let origin_in_dpxs = glyph.origin_in_dpxs;
        let bounds_in_dpxs = TextRect::new(
            Point::new(
                origin_in_dpxs.x - atlas_image_padding as f32,
                -origin_in_dpxs.y - atlas_image_size.height as f32 + (atlas_image_padding as f32),
            ),
            Size::new(
                atlas_image_size.width as f32,
                atlas_image_size.height as f32,
            ),
        );
        let bounds_in_lpxs = bounds_in_dpxs.apply_transform(
            Transform::from_scale_uniform(font_size_in_lpxs / glyph.dpxs_per_em * self.font_scale)
                .translate(origin_in_lpxs.x, origin_in_lpxs.y),
        );

        self.rect_pos = vec2(bounds_in_lpxs.origin.x, bounds_in_lpxs.origin.y)
            + vec2(0.0, self.temp_y_shift * font_size_in_lpxs);
        self.rect_size = vec2(bounds_in_lpxs.size.width, bounds_in_lpxs.size.height);
        if let Some(color) = color {
            self.color = vec4(
                color.r as f32,
                color.g as f32,
                color.b as f32,
                color.a as f32,
            ) / 255.0;
        }
        self.texture_index = texture_index;
        self.atlas_plane = glyph.atlas_plane as f32;
        self.t_min = vec2(t_min.x, t_min.y);
        self.t_max = vec2(t_max.x, t_max.y);
        let slice = self.draw_vars.as_slice();

        output.extend_from_slice(slice);
        self.glyph_depth += 0.000001;
        self.char_index += 1.0;
    }

    /// Resets the character index counter to 0. Call this before drawing text
    /// when you want to track character positions for animation effects.
    pub fn reset_char_index(&mut self) {
        self.char_index = 0.0;
    }

    /// Sets the total_chars instance value on all instances in the area after drawing is complete.
    /// This allows the shader to know how many characters are in the buffer
    /// for fade-in animation effects.
    pub fn set_total_chars(&mut self, cx: &mut Cx, total: f32) {
        self.draw_vars
            .set_instance_on_area(cx, live_id!(total_chars), &[total]);
    }

    /// What this text drew (hit-tests and widget queries read it).
    pub fn area(&self) -> Area {
        self.draw_vars.area
    }

    #[track_caller]
    pub fn redraw_areas(&self, cx: &mut Cx) {
        self.draw_vars.area.redraw(cx);
    }

    #[track_caller]
    pub fn redraw(&self, cx: &mut Cx) {
        self.redraw_areas(cx);
    }

    pub fn new_draw_call(&mut self, cx: &mut Cx2d) {
        self.update_draw_vars(cx);
        cx.new_draw_call(&self.draw_vars);
    }

    pub fn append_to_draw_call(&self, cx: &mut Cx2d) {
        cx.append_to_draw_call(&self.draw_vars);
    }

    pub fn get_aa_pad_px(&self, cx: &mut Cx) -> f32 {
        let mut value = [0.0];
        self.draw_vars
            .get_uniform(cx, live_id!(aa_pad_px), &mut value);
        value[0]
    }
}


#[derive(Debug, Clone, Script, ScriptHook)]
pub struct TextStyle {
    #[live]
    pub font_family: FontFamily,
    #[live(10.0)]
    pub font_size: f32,
    #[live(1.0)]
    pub line_spacing: f32,
    /// A vertical offset applied when drawing text, as a fraction of the font size.
    /// Positive values shift text downward, useful for aligning baselines when
    /// mixing fonts with different vertical metrics (e.g., code font with regular text).
    #[live(0.0)]
    pub top_drop: f32,
}

#[derive(Debug, Clone, Script, ScriptHook)]
pub struct FontMember {
    #[live]
    pub res: Option<ScriptHandleRef>,
    #[live]
    pub asc: f32,
    #[live]
    pub desc: f32,
    /// Positive values map to the OpenType `wght` axis. `0.0` keeps the font default.
    #[live(0.0)]
    pub weight: f32,
    /// Any other variable-font axes, as `"wdth 125 slnt -8 GRAD 50"`
    /// (four-letter tag, value; axes the font lacks are ignored).
    #[live]
    pub axes: String,
    /// `1` = CJK and `2` = emoji. These members are requested only after a
    /// real glyph miss; zero keeps the ordinary eager behavior.
    #[live(0.0)]
    pub lazy: f32,
}

#[derive(Debug, Clone, Script, PartialEq)]
pub struct FontFamily {
    #[rust]
    id: LiveId,
    #[rust]
    members: Vec<FontMemberDef>,
    #[rust]
    diagnostic_role: String,
    #[rust]
    diagnostic_set: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FontMemberDef {
    handle: ScriptHandle,
    resource_path: String,
    id: String,
    asc: f32,
    desc: f32,
    weight: f32,
    /// Variable-axis values as (tag, value); `wght` rides in `weight`.
    variations: Vec<(u32, f32)>,
    lazy: Option<LazyFontFamily>,
}

/// Axis values from `"wdth 125 slnt -8"`: pairs of a tag of up to four
/// characters (padded with spaces) and a number; malformed pairs are
/// skipped.
pub fn parse_font_axes(text: &str) -> Vec<(u32, f32)> {
    let words: Vec<&str> = text.split_whitespace().collect();
    words
        .chunks(2)
        .filter_map(|pair| {
            let [tag, value] = pair else { return None };
            let b = tag.as_bytes();
            if b.is_empty() || b.len() > 4 {
                return None;
            }
            let mut t = [b' '; 4];
            t[..b.len()].copy_from_slice(b);
            let v: f32 = value.parse().ok()?;
            v.is_finite().then_some((u32::from_be_bytes(t), v))
        })
        .collect()
}

impl FontFamily {
    fn to_font_family_id(&self) -> FontFamilyId {
        (self.id.0).into()
    }

    /// The first member's ascender adjustment (`asc`, in ems; negative
    /// raises the glyphs): where the layout puts the baseline against the
    /// font's own ascender, for drawing the same text another way (glyph
    /// outlines) in the same place.
    pub fn ascender_fudge_in_ems(&self) -> f32 {
        self.members.first().map_or(0.0, |member| member.asc)
    }

    /// This family read at other variable-axis values: every member takes
    /// `axes` (`(tag, value)`, e.g. `wdth` 125.0; a `wght` value is the
    /// member's weight), on top of its own. A font without an axis ignores
    /// it. The result is its own family (its own id and font cache), so a
    /// caller animating an axis should step its values (whole units) and
    /// keep the families it made.
    pub fn with_axes(&self, axes: &[(u32, f32)]) -> FontFamily {
        const WGHT: u32 = u32::from_be_bytes(*b"wght");
        let mut out = self.clone();
        for member in &mut out.members {
            for &(tag, value) in axes {
                if !value.is_finite() {
                    continue;
                }
                if tag == WGHT {
                    member.weight = value;
                    continue;
                }
                match member.variations.iter_mut().find(|(t, _)| *t == tag) {
                    Some(slot) => slot.1 = value,
                    None => member.variations.push((tag, value)),
                }
            }
            member.variations.sort_by_key(|(t, _)| *t);
        }
        out.id = family_id_of(&out.members);
        out
    }

    /// Stable logical member names in fallback order.
    pub fn member_ids(&self) -> impl ExactSizeIterator<Item = &str> {
        self.members.iter().map(|member| member.id.as_str())
    }

    fn update_font_definitions(&self, cx: &mut Cx, fonts: &mut Fonts) {
        let mut font_ids = Vec::new();
        let mut expected_member_count = 0;
        let family_id = self.to_font_family_id();

        for member in &self.members {
            if let Some(lazy) = member.lazy {
                if !fonts.lazy_font_is_requested(family_id, lazy) {
                    continue;
                }
            }
            let font_id = font_member_font_id(member);
            // A member whose resource this process can never read (a font
            // the build's font set leaves out, an asset missing from the
            // package) is out of the family for good. Counted, it would keep
            // the family incomplete, and an incomplete family is redefined
            // on every frame: the layout cache emptied, every text laid out
            // again, the missing asset opened again.
            if fonts.is_font_unavailable(font_id) {
                continue;
            }

            if !fonts.is_font_known(font_id) {
                let font_data = cx.get_resource_font_bytes_by_path(&member.resource_path);
                crate::trace!(
                    "font",
                    "family={:?} member={} path={:?} bytes={:?}",
                    self.id.0,
                    member.id,
                    member.resource_path,
                    font_data.as_ref().map(|d| d.len())
                );

                if let Some(data) = font_data {
                    if std::env::var_os("MAKEPAD_TRACE_FONT_LOAD").is_some() {
                        log!("font load: member={} path={} id={:?} bytes={}", member.id, member.resource_path, font_id, data.len());
                    }
                    if let Some(family) = member.lazy {
                        log!("lazy font family loaded: {:?} ({})", family, member.id);
                    }
                    fonts.define_font(
                        font_id,
                        FontDefinition {
                            data,
                            index: 0,
                            ascender_fudge_in_ems: member.asc,
                            descender_fudge_in_ems: member.desc,
                            weight: font_member_weight(member),
                            variations: member.variations.clone(),
                        },
                    );
                // Two different absences, and only one is a mistake. A build that
                // leaves a font out on purpose says so through the resource table:
                // the family renders without it, and that is a log. A font nobody
                // declared at all shows as boxes on screen, so it is an error that
                // says how to fix it.
                } else if cx.script_resource_unavailable(&member.resource_path) {
                    // Not packaged with this app: the family renders without
                    // it (dropped, not asked for again every frame), and it
                    // is said once, loudly, with the fix.
                    if fonts.note_font_unavailable(font_id) && fonts.note_missing_font(&member.resource_path) {
                        error!(
                            "font {:?} (member {}) is not packaged with this app, so its text will show as boxes. \
                            Declare it in `app_main!`, e.g. `font_assets: [MATH_VIEW_FONT_ASSET]`, \
                            `INTER_FONT_ASSET` or `ROBOTO_FLEX_FONT_ASSET`, or a literal resource path.",
                            member.resource_path, member.id
                        );
                    }
                    continue;
                } else if fonts.note_missing_font(&member.resource_path) {
                    error!(
                        "font {:?} (member {}) is not packaged with this app, so its text will show as boxes. \
                        Declare it in `app_main!`, e.g. `font_assets: [MATH_VIEW_FONT_ASSET]`, \
                        `INTER_FONT_ASSET` or `ROBOTO_FLEX_FONT_ASSET`, or a literal resource path.",
                        member.resource_path, member.id
                    );
                }
            }
            expected_member_count += 1;

            if fonts.is_font_known(font_id) {
                font_ids.push(font_id);
            }
        }

        crate::trace!(
            "font",
            "family={:?} defined with {}/{} members",
            self.id.0,
            font_ids.len(),
            expected_member_count
        );
        fonts.set_font_family_definition(
            family_id,
            FontFamilyDefinition {
                font_ids,
                expected_member_count,
                diagnostics: FontDiagnostics {
                    role: self.diagnostic_role.clone(),
                    set: self.diagnostic_set.clone(),
                    tried: self
                        .members
                        .iter()
                        .filter(|member| {
                            member
                                .lazy
                                .map_or(true, |lazy| fonts.lazy_font_is_requested(family_id, lazy))
                        })
                        .map(|member| member.id.clone())
                        .collect(),
                },
            },
        );
    }

    fn ensure_fonts_loaded(&self, cx: &mut Cx) {
        CxDraw::lazy_construct_fonts(cx);

        let family_id = self.to_font_family_id();
        let fonts = cx.get_global::<Rc<RefCell<Fonts>>>().clone();
        fonts.borrow_mut().sync_unavailable_fonts(cx.script_resource_generation());
        let expected_member_count = {
            let fonts_ref = fonts.borrow();
            self.members
                .iter()
                .filter(|member| {
                    member
                        .lazy
                        .map_or(true, |lazy| fonts_ref.lazy_font_is_requested(family_id, lazy))
                        && !fonts_ref.is_font_unavailable(font_member_font_id(member))
                })
                .count()
        };

        {
            let fonts_ref = fonts.borrow();
            if fonts_ref.is_font_family_complete(family_id, expected_member_count) {
                return;
            }
        }

        let handles = {
            let fonts_ref = fonts.borrow();
            self.members
                .iter()
                .filter(|member| {
                    member
                        .lazy
                        .map_or(true, |lazy| fonts_ref.lazy_font_is_requested(family_id, lazy))
                })
                .map(|member| member.resource_path.as_str())
                .collect::<Vec<_>>()
        };
        for path in handles {
            load_font_resource(cx, path);
        }
        {
            let fonts_ref = fonts.borrow();
            if fonts_ref.is_font_family_complete(family_id, expected_member_count) {
                return;
            }
        }

        let mut fonts_ref = fonts.borrow_mut();
        self.update_font_definitions(cx, &mut fonts_ref);
    }

    /// Whether a lazy face was asked for (and, where the bytes are at hand,
    /// is in the family now).
    fn request_lazy_fonts_for_glyph_miss(
        &self,
        cx: &mut Cx,
        text: &str,
        has_missing_glyph: bool,
    ) -> bool {
        let fonts = cx.get_global::<Rc<RefCell<Fonts>>>().clone();
        let family_id = self.to_font_family_id();
        let handles = {
            let mut fonts_ref = fonts.borrow_mut();
            self.members
                .iter()
                .filter_map(|member| {
                    let family = member.lazy?;
                    fonts_ref
                        .take_lazy_font_request(
                            family_id,
                            family,
                            text,
                            has_missing_glyph,
                        )
                        .then_some(member.resource_path.as_str())
                })
                .collect::<Vec<_>>()
        };
        if handles.is_empty() {
            return false;
        }
        for path in handles {
            load_font_resource(cx, path);
        }

        self.update_font_definitions(cx, &mut fonts.borrow_mut());
        // Native file reads may have completed above; wasm will redraw again
        // when the HTTP response populates the resource.
        cx.redraw_all();
        true
    }
}

/// Every family ends with the platform's CJK and emoji faces
/// (`LAZY_FONT_FALLBACKS`), unless it names one of that family or that face
/// itself: a family picked for its Latin face (a code font, a terminal's)
/// draws Chinese and emoji like the rest of the UI instead of empty boxes.
/// They are lazy members: nothing is opened until a CJK or emoji glyph misses
/// in the family's own faces, so a family that never meets one costs nothing,
/// and one whose own faces cover it (the International UI chain) never loads
/// them.
fn append_lazy_fallbacks(vm: &mut ScriptVm, members: &mut Vec<FontMemberDef>) {
    if members.is_empty() {
        return;
    }
    use crate::makepad_platform::script::res::crate_resource_abs_path;
    for fallback in LAZY_FONT_FALLBACKS {
        if members.iter().any(|member| member.lazy == Some(fallback.family)) {
            continue;
        }
        let named = LAZY_FONT_FACES
            .iter()
            .filter(|face| face.family == fallback.family)
            .filter_map(|face| crate_resource_abs_path(vm, face.asset.resource_path))
            .any(|path| members.iter().any(|member| member.resource_path == path));
        if named {
            continue;
        }
        let Some(resource_path) = crate_resource_abs_path(vm, fallback.asset.resource_path) else {
            continue;
        };
        members.push(FontMemberDef {
            handle: ScriptHandle::ZERO,
            resource_path,
            id: fallback.asset.id.to_string(),
            asc: fallback.asset.ascender,
            desc: fallback.asset.descender,
            weight: fallback.asset.weight,
            variations: Vec::new(),
            lazy: Some(fallback.family),
        });
    }
}

fn font_member_weight(member: &FontMemberDef) -> Option<f32> {
    if member.weight.is_finite() && member.weight > 0.0 {
        Some(member.weight)
    } else {
        None
    }
}

/// A family's id from its complete ordered members (resources, metrics and
/// axis values), so identical families share a cache and different ones
/// never alias.
fn family_id_of(members: &[FontMemberDef]) -> LiveId {
    let mut hasher = DefaultHasher::new();
    for member in members {
        font_member_font_id(member).hash(&mut hasher);
        (member.lazy.map(|v| v as u32)).hash(&mut hasher);
    }
    LiveId(hasher.finish())
}

fn font_member_font_id(member: &FontMemberDef) -> FontId {
    let mut hasher = DefaultHasher::new();
    member.resource_path.hash(&mut hasher);
    member.asc.to_bits().hash(&mut hasher);
    member.desc.to_bits().hash(&mut hasher);
    member.weight.to_bits().hash(&mut hasher);
    for (tag, value) in &member.variations {
        tag.hash(&mut hasher);
        value.to_bits().hash(&mut hasher);
    }
    FontId::from(hasher.finish())
}

fn row_span_x_bounds_in_lpxs(
    row: &LaidoutRow,
    is_first_row: bool,
    _is_last_row: bool,
) -> (f32, f32) {
    let start_x_in_lpxs = if is_first_row {
        row.glyphs
            .first()
            .map(|glyph| glyph.origin_in_lpxs.x)
            .unwrap_or(row.width_in_lpxs)
    } else {
        0.0
    };
    let end_x_in_lpxs = row.width_in_lpxs;
    (start_x_in_lpxs, end_x_in_lpxs.max(start_x_in_lpxs))
}

impl TextStyle {
    pub fn font_family_id(&self) -> FontFamilyId {
        self.font_family.to_font_family_id()
    }

    pub fn ensure_fonts_loaded(&self, cx: &mut Cx) {
        self.font_family.ensure_fonts_loaded(cx);
    }
}

impl ScriptHook for FontFamily {
    fn on_custom_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) -> bool {
        let Some(obj) = value.as_object() else {
            return false;
        };

        self.members.clear();
        let selected_set = vm.cx().font_set().as_str().to_string();
        let map = vm.heap().map_entries(obj);
        self.diagnostic_role = map
            .iter().find(|e| e.key == id!(_font_role).into())
            .and_then(|entry| entry.value.as_id())
            .and_then(|id| id.as_string(|name| name.map(str::to_owned)))
            .unwrap_or_else(|| "custom".to_string());
        self.diagnostic_set = map
            .iter().find(|e| e.key == id!(_font_set).into())
            .and_then(|entry| entry.value.as_id())
            .and_then(|id| id.as_string(|name| name.map(str::to_owned)))
            .unwrap_or(selected_set);

        let len = vm.heap().vec_len(obj);
        for i in 0..len {
            let kv = vm.heap().vec_key_value(obj, i, NoTrap);
            let member = FontMember::script_from_value(vm, kv.value);
            if let Some(ref handle_ref) = member.res {
                let heap_key = vm.heap_key();
                let Some(resource_path) = vm.cx().script_data.resources
                    .path_for_handle(heap_key, handle_ref.as_handle()) else {
                    error!("Font resource is not registered in its owning script heap");
                    continue;
                };
                self.members.push(FontMemberDef {
                    handle: handle_ref.as_handle(),
                    resource_path,
                    id: kv
                        .key
                        .as_id()
                        .and_then(|id| id.as_string(|name| name.map(str::to_owned)))
                        .unwrap_or_else(|| format!("resource_{}", handle_ref.as_handle().index())),
                    asc: member.asc,
                    desc: member.desc,
                    weight: member.weight,
                    variations: {
                        let mut v: Vec<(u32, f32)> = parse_font_axes(&member.axes).into_iter().filter(|(t, _)| *t != u32::from_be_bytes(*b"wght")).collect();
                        v.sort_by_key(|(t, _)| *t);
                        v
                    },
                    lazy: match member.lazy as u32 {
                        1 => Some(LazyFontFamily::Cjk),
                        2 => Some(LazyFontFamily::Emoji),
                        _ => None,
                    },
                });
            } else {
                crate::trace!("font", "family={:?} member {} dropped: res is None", self.id.0, i);
            }
        }

        append_lazy_fallbacks(vm, &mut self.members);

        // Object and handle indices are local to a script heap. Font caches
        // belong to Cx, so identify the complete ordered family by its actual
        // resources and metrics. Identical families can safely share a cache
        // across isolates, while different ones can never alias by index.
        self.id = family_id_of(&self.members);

        // Don't eagerly register fonts here. Font registration is deferred
        // to ensure_fonts_loaded() which is called at draw time.
        // This avoids redundant work when the same FontFamily is applied
        // to hundreds of widgets.

        true
    }
}

#[cfg(test)]
mod axes_tests {
    use super::*;

    #[test]
    fn axes_parse_and_make_their_own_family() {
        let wdth = u32::from_be_bytes(*b"wdth");
        let grad = u32::from_be_bytes(*b"GRAD");
        assert_eq!(parse_font_axes("wdth 125 GRAD -50 bad x toolong 3"), vec![(wdth, 125.0), (grad, -50.0)]);
        let member = FontMemberDef { resource_path: "a.ttf".into(), weight: 400.0, ..Default::default() };
        let family = FontFamily { id: family_id_of(&[member.clone()]), members: vec![member], diagnostic_role: String::new(), diagnostic_set: String::new() };
        let wide = family.with_axes(&[(wdth, 125.0), (u32::from_be_bytes(*b"wght"), 900.0)]);
        assert_ne!(wide.id, family.id);
        assert_eq!(wide.members[0].weight, 900.0);
        assert_eq!(wide.members[0].variations, vec![(wdth, 125.0)]);
        assert_eq!(family.with_axes(&[(wdth, 125.0), (u32::from_be_bytes(*b"wght"), 900.0)]).id, wide.id, "the same values give the same family");
        assert_ne!(font_member_font_id(&wide.members[0]), font_member_font_id(&family.members[0]));
    }
}

#[cfg(test)]
mod tests {
    use super::{DrawText, TextOverflow};
    use crate::{
        cx_2d::Cx2d,
        cx_draw::CxDraw,
        makepad_platform::{dvec2, Cx, DrawEvent, Inset, ScriptNew},
        turtle::{FitBound, Layout, Size, Walk},
    };
    use crate::makepad_platform::{live_id, vec4, LiveId, ScriptVmCx};

    #[test]
    fn draw_text_size_stays_16_byte_aligned() {
        assert_eq!(std::mem::size_of::<DrawText>() % 16, 0);
    }

    #[test]
    fn ellipsis_fit_bound_keeps_the_below_ellipsis_relocation_path() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut draw_text = cx.with_vm(|vm| {
            crate::script_mod(vm);
            DrawText::script_new_with_default(vm)
        });
        draw_text.text_overflow = TextOverflow::Ellipsis;
        draw_text.font_scale = 1.0;

        let event = DrawEvent::default();
        let mut draw = CxDraw::new(&mut cx, &event);
        let mut cx = Cx2d::new(&mut draw);
        cx.begin_root_turtle(dvec2(300.0, 100.0), Layout::default());
        cx.begin_turtle(
            Walk {
                width: Size::Fit {
                    min: None,
                    max: Some(FitBound::Abs(120.0)),
                },
                height: Size::fit(),
                ..Default::default()
            },
            Layout::default().with_padding_left(5.0).with_padding_right(5.0),
        );

        // Label folds its padding into this margin before forwarding the walk.
        let walk = Walk {
            margin: Inset {
                left: 10.0,
                right: 10.0,
                ..Default::default()
            },
            width: Size::Fit {
                min: None,
                max: Some(FitBound::Abs(100.0)),
            },
            height: Size::fit(),
            ..Default::default()
        };
        assert_eq!(draw_text.max_layout_width_for_walk(&mut cx, walk), None);
        // The Size::Fit max is a margin-box limit: walk_max_width removes the
        // 20px label margin, then the enclosing turtle's padding is removed once.
        assert_eq!(
            draw_text.fit_layout_width_for_walk(&cx, walk, Some(60.0)),
            Some(70.0)
        );
        // Leaving this unbounded is what keeps the enclosing flow's relocation
        // and hide-then-ellipsis behavior reachable.
        assert_eq!(
            draw_text.fit_layout_width_for_walk(&cx, walk, Some(80.0)),
            None
        );

        let current_bound_fallback = Walk::fit();
        assert_eq!(
            draw_text.fit_layout_width_for_walk(&cx, current_bound_fallback, None),
            Some(110.0)
        );
        cx.end_turtle();
        cx.end_turtle();
    }

    fn read_instance(draw_vars: &super::DrawVars, cx: &mut Cx, id: LiveId) -> [f32; 4] {
        let mut value = [0.0; 4];
        draw_vars.get_instance(cx, id, &mut value);
        value
    }

    #[test]
    fn draw_text_color_is_visible_through_instance_slice() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let mut draw_text = DrawText::script_new_with_default(vm);
            draw_text.color = vec4(0.11, 0.22, 0.33, 0.44);

            let packed =
                vm.with_cx_mut(|cx| read_instance(&draw_text.draw_vars, cx, live_id!(color)));

            assert_eq!(packed, [0.11, 0.22, 0.33, 0.44]);
        });
    }

}

/// Reads a font member's resource unless `get_resource_font_bytes_by_path` can map it.
/// Local files and stored Android assets bypass the resource table's duplicate heap copy.
fn load_font_resource(cx: &mut Cx, path: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    if std::path::Path::new(path).is_file() {
        return;
    }
    #[cfg(target_os = "android")]
    if cx.get_resource_font_asset_bytes_by_path(path).is_some() {
        return;
    }
    cx.load_script_resource_by_path(path);
}
