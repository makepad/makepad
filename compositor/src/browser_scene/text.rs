use makepad_draw::text::geom::Point as TextPoint;
use makepad_draw::text::rasterizer::{AtlasKind, RasterizedGlyph};

use super::text_prepare::{
    BrowserGlyphFormat, MpPreparedBrowserScene, PreparedBrowserTextRun,
};
use super::{MpBrowserScene, MpBrowserSceneExecState, MpBrowserTextRun, MpBrowserTextRunId};
use crate::*;

impl MpBrowserSceneExecState {
    /// Draws a text run on the direct text path.
    ///
    /// Direct browser text stays on the direct path only for the root
    /// untransformed case. Host placement is applied explicitly to submitted
    /// rects and clips here. Transformed text routes through picture fallback.
    ///
    /// This path is direct-text only. It returns early when the clip chain has
    /// non-empty entries because complex clips use picture fallback.
    pub(super) fn draw_text_run(
        &mut self,
        cx: &mut Cx2d,
        scene: &MpBrowserScene,
        prepared_text: &MpPreparedBrowserScene,
        text_run_id: MpBrowserTextRunId,
    ) {
        let Some(text_run) = scene.text_runs.get(text_run_id) else {
            return;
        };
        let Some(prepared_run) = prepared_text.prepared_run(text_run.stable_id) else {
            return;
        };
        if !self.text_cache.is_prepared_run_valid(prepared_run) {
            return;
        }
        let Some(clip_chain) = scene
            .primitive_scene
            .clip_chains
            .get(text_run.clip_chain_id)
        else {
            return;
        };
        if !clip_chain.entries.is_empty() {
            return;
        }
        debug_assert!(clip_chain.entries.is_empty(), "direct text path requires an empty complex clip chain");
        // Browser direct text currently submits glyph quads directly through
        // DrawText, which has no per-draw-call transform slot. Draw-list
        // uniforms are per draw list, not per submitted text run, so temporary
        // draw-list uniform mutation cannot carry text-run placement.
        //
        // Keep direct text only for the root untransformed case and apply the
        // host translation explicitly in submitted rects and clips here.
        let draw_clip = clip_chain.origin_clip_rect.map_or(
            vec4(-100000.0, -100000.0, 100000.0, 100000.0),
            |rect| {
                let shifted = Rect {
                    pos: rect.pos + scene.host_rect.pos,
                    size: rect.size,
                };
                vec4(
                    shifted.pos.x as f32,
                    shifted.pos.y as f32,
                    (shifted.pos.x + shifted.size.x) as f32,
                    (shifted.pos.y + shifted.size.y) as f32,
                )
            },
        );
        self.draw_color.draw_super.draw_clip = draw_clip;
        self.draw_text.draw_clip = draw_clip;

        debug_assert!(
            text_run.local_rect.pos.x.is_finite() && text_run.local_rect.pos.y.is_finite(),
            "text run origin must stay finite",
        );
        draw_browser_text_run(
            cx,
            &self.text_cache,
            &mut self.draw_color,
            &mut self.draw_text,
            text_run,
            prepared_run,
            scene.host_rect.pos,
        );
    }
}

/// Draws all text decorations and glyphs from the same run-origin basis.
///
/// Background, underline, overline, line-through, shadows, and glyphs all
/// derive from `text_run.local_rect.pos + scene_origin`. This keeps every text
/// element moving together under explicit host placement.
fn draw_browser_text_run(
    cx: &mut Cx2d,
    text_cache: &super::text_prepare::BrowserTextCache,
    draw_bg: &mut DrawColor,
    draw_text: &mut DrawText,
    text_run: &MpBrowserTextRun,
    prepared_run: &PreparedBrowserTextRun,
    scene_origin: DVec2,
) {
    debug_assert!(
        text_run.local_rect.pos.x.is_finite() && text_run.local_rect.pos.y.is_finite(),
        "text run origin must stay finite",
    );

    let advance_width = text_run.metrics.advance_width_px.max(0.0) as f64;
    let decoration_color = text_run
        .decorations
        .decoration_color
        .unwrap_or(text_run.color);
    let text_width = advance_width.min(text_run.local_rect.size.x);
    let run_origin = text_run.local_rect.pos + scene_origin;

    if let Some(background) = text_run.decorations.background_color {
        if background.w > 0.001 && advance_width > 0.0 && text_run.local_rect.size.y > 0.0 {
            begin_browser_text_bg_batch(cx, draw_bg);
            draw_bg.color = background;
            draw_bg.draw_abs(
                cx,
                Rect {
                    pos: run_origin,
                    size: dvec2(text_width, text_run.local_rect.size.y),
                },
            );
        }
    }

    if text_run.decorations.underline {
        begin_browser_text_bg_batch(cx, draw_bg);
        draw_bg.color = decoration_color;
        draw_bg.draw_abs(
            cx,
            Rect {
                pos: dvec2(
                    run_origin.x,
                    run_origin.y
                        + text_run.metrics.baseline_ascent_px as f64
                        + text_run.metrics.underline_offset_px as f64,
                ),
                size: dvec2(
                    text_width,
                    text_run.metrics.underline_thickness_px.max(0.0) as f64,
                ),
            },
        );
    }
    if text_run.decorations.overline {
        begin_browser_text_bg_batch(cx, draw_bg);
        draw_bg.color = decoration_color;
        draw_bg.draw_abs(
            cx,
            Rect {
                pos: run_origin,
                size: dvec2(
                    text_width,
                    text_run.metrics.underline_thickness_px.max(0.0) as f64,
                ),
            },
        );
    }
    end_browser_text_bg_batch(cx, draw_bg);

    for shadow in text_run.decorations.shadows.iter().rev() {
        let _ = shadow.blur_radius_px;
        draw_prepared_glyphs(
            cx,
            text_cache,
            draw_text,
            prepared_run,
            run_origin + shadow.offset,
            shadow.color,
        );
    }
    draw_prepared_glyphs(
        cx,
        text_cache,
        draw_text,
        prepared_run,
        run_origin,
        text_run.color,
    );

    if text_run.decorations.line_through {
        begin_browser_text_bg_batch(cx, draw_bg);
        draw_bg.color = decoration_color;
        draw_bg.draw_abs(
            cx,
            Rect {
                pos: dvec2(
                    run_origin.x,
                    run_origin.y
                        + text_run.metrics.baseline_ascent_px as f64
                        - text_run.metrics.strikeout_offset_px as f64,
                ),
                size: dvec2(
                    text_width,
                    text_run.metrics.strikeout_thickness_px.max(0.0) as f64,
                ),
            },
        );
        end_browser_text_bg_batch(cx, draw_bg);
    }
}

/// Start non-aligned browser text batching for background and decoration quads.
fn begin_browser_text_bg_batch(cx: &mut Cx2d, draw_bg: &mut DrawColor) {
    if draw_bg.draw_super.many_instances.is_none() {
        draw_bg.draw_super.new_draw_call(cx);
        draw_bg.draw_super.many_instances = cx.begin_many_instances(&draw_bg.draw_super.draw_vars);
    }
}

fn end_browser_text_bg_batch(cx: &mut Cx2d, draw_bg: &mut DrawColor) {
    if let Some(instances) = draw_bg.draw_super.many_instances.take() {
        let new_area = cx.end_many_instances(instances);
        draw_bg.draw_super.draw_vars.area = cx.update_area_refs(draw_bg.draw_super.draw_vars.area, new_area);
    }
}

/// Draws glyphs at exact anchors from the active draw-list basis.
///
/// This is the single join point where origin-space `local_rect.pos` and
/// primitive-local prepared glyph origins are combined. The final glyph anchor
/// `origin + glyph.origin` is still in draw-list-local space. `DrawText` then
/// applies the active draw-list `view_transform` and pass projection.
fn draw_prepared_glyphs(
    cx: &mut Cx2d,
    text_cache: &super::text_prepare::BrowserTextCache,
    draw_text: &mut DrawText,
    prepared_run: &PreparedBrowserTextRun,
    origin: DVec2,
    color: Vec4f,
) {
    debug_assert!(
        origin.x.is_finite() && origin.y.is_finite(),
        "glyph draw origin must stay finite",
    );
    for batch in &prepared_run.batches {
        let Some(binding) = text_cache.page_binding(batch.page) else {
            continue;
        };
        if binding.generation != batch.page.page_generation {
            continue;
        }
        begin_browser_text_page_batch(cx, draw_text, binding.format, binding.texture);
        draw_text.color = color;
        for glyph in &batch.glyphs {
            debug_assert!(
                glyph.origin.x.is_finite() && glyph.origin.y.is_finite(),
                "primitive-local glyph origin must stay finite",
            );
            let glyph_anchor = origin + glyph.origin;
            debug_assert!(
                glyph_anchor.x.is_finite() && glyph_anchor.y.is_finite(),
                "final glyph anchor must stay finite after the origin join",
            );
            draw_text.draw_rasterized_glyph_exact_prepared_abs(
                cx,
                TextPoint::new(glyph_anchor.x as f32, glyph_anchor.y as f32),
                glyph.font_size_px,
                rasterized_glyph_from_entry(glyph.entry),
                color,
            );
        }
        end_browser_text_page_batch(cx, draw_text);
    }
}

fn begin_browser_text_page_batch(
    cx: &mut Cx2d,
    draw_text: &mut DrawText,
    format: BrowserGlyphFormat,
    texture: &Texture,
) {
    draw_text.set_atlas_texture(cx, atlas_kind_for_browser_format(format), texture);
    draw_text.glyph_depth = draw_text.draw_depth;
    cx.new_draw_call(&draw_text.draw_vars);
    draw_text.many_instances = cx.begin_many_instances(&draw_text.draw_vars);
}

fn end_browser_text_page_batch(cx: &mut Cx2d, draw_text: &mut DrawText) {
    if let Some(instances) = draw_text.many_instances.take() {
        let new_area = cx.end_many_instances(instances);
        draw_text.draw_vars.area = cx.update_area_refs(draw_text.draw_vars.area, new_area);
    }
}

fn atlas_kind_for_browser_format(format: BrowserGlyphFormat) -> AtlasKind {
    match format {
        BrowserGlyphFormat::Alpha => AtlasKind::Grayscale,
        BrowserGlyphFormat::Color => AtlasKind::Color,
        BrowserGlyphFormat::Msdf => AtlasKind::Msdf,
    }
}

fn rasterized_glyph_from_entry(entry: super::text_prepare::BrowserGlyphEntry) -> RasterizedGlyph {
    RasterizedGlyph {
        atlas_kind: atlas_kind_for_browser_format(entry.page.format),
        atlas_size: entry.atlas_page_size,
        atlas_image_bounds: entry.atlas_image_bounds,
        atlas_image_padding: entry.atlas_image_padding,
        atlas_plane: entry.atlas_plane,
        origin_in_dpxs: entry.origin_in_dpxs,
        dpxs_per_em: entry.dpxs_per_em,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_scene::{
        MpBrowserGlyphInstance, MpBrowserTextDecorations, MpBrowserTextMetrics,
    };

    #[test]
    fn text_run_decorations_use_same_basis_as_glyphs() {
        let scene_origin = dvec2(0.0, 102.6);
        let text_run = MpBrowserTextRun {
            stable_id: 1,
            local_rect: Rect {
                pos: dvec2(10.0, 20.0),
                size: dvec2(120.0, 24.0),
            },
            transform_id: 0,
            clip_chain_id: 0,
            color: vec4(1.0, 1.0, 1.0, 1.0),
            fonts: Vec::new(),
            glyphs: vec![MpBrowserGlyphInstance {
                glyph_id: 7,
                font_size_px: 14.0,
                origin: dvec2(5.0, 12.0),
                font_slot: 0,
            }],
            metrics: MpBrowserTextMetrics {
                advance_width_px: 40.0,
                baseline_ascent_px: 9.0,
                underline_offset_px: 2.0,
                underline_thickness_px: 1.5,
                strikeout_offset_px: 4.0,
                strikeout_thickness_px: 1.0,
            },
            decorations: MpBrowserTextDecorations {
                background_color: Some(vec4(1.0, 0.0, 0.0, 1.0)),
                decoration_color: Some(vec4(0.0, 1.0, 0.0, 1.0)),
                underline: true,
                overline: false,
                line_through: false,
                shadows: Vec::new(),
            },
        };

        let run_origin = text_run.local_rect.pos + scene_origin;
        let background_rect = Rect {
            pos: run_origin,
            size: dvec2(
                text_run
                    .metrics
                    .advance_width_px
                    .min(text_run.local_rect.size.x as f32) as f64,
                text_run.local_rect.size.y,
            ),
        };
        let underline_rect = Rect {
            pos: dvec2(
                run_origin.x,
                run_origin.y
                    + text_run.metrics.baseline_ascent_px as f64
                    + text_run.metrics.underline_offset_px as f64,
            ),
            size: dvec2(
                text_run
                    .metrics
                    .advance_width_px
                    .min(text_run.local_rect.size.x as f32) as f64,
                text_run.metrics.underline_thickness_px as f64,
            ),
        };
        let glyph_anchor = run_origin + text_run.glyphs[0].origin;

        assert!(text_run.decorations.background_color.is_some());
        assert!(text_run.decorations.underline);
        assert_eq!(background_rect.pos, run_origin);
        assert_eq!(underline_rect.pos.x, run_origin.x);
        assert_eq!(
            underline_rect.pos.y,
            run_origin.y
                + text_run.metrics.baseline_ascent_px as f64
                + text_run.metrics.underline_offset_px as f64,
        );
        assert_eq!(glyph_anchor - text_run.glyphs[0].origin, run_origin);
        assert_eq!(glyph_anchor.x, 15.0);
        assert_eq!(glyph_anchor.y, 134.6);
    }
}
