use crate::{
    project_rule_on_grid, project_staff_rules_on_grid, tessellate_ribbon, DashPattern,
    GlyphItem, HighlightRole, InstanceBatch, LinearRgba, MusicFontRef, OverlayCommand, PaintKind,
    PageLodPlan, PlannedItemRef, Point, Primitive, Rect, RenderPlan, RuleKind, ScorePalette,
    SmuflGlyph, TextFontRef, TextRun, TileKey, Transform,
};
use makepad_draw::{
    rect as mp_rect, vec4, Cx2d, DrawGlyph, DrawText, DrawVector, Vec2d,
};
use makepad_draw::shader::draw_glyph::GlyphShapeId;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug, PartialEq)]
pub enum GlyphOutlineCommand {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    QuadTo(f32, f32, f32, f32),
    CubicTo(f32, f32, f32, f32, f32, f32),
    Close,
}

/// Backend-neutral font outline. Coordinates are OpenType font units, y-up.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphOutline {
    pub units_per_em: u16,
    pub commands: Arc<[GlyphOutlineCommand]>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct GlyphKey {
    font: MusicFontRef,
    glyph: SmuflGlyph,
}

#[derive(Clone, Copy, Debug)]
struct RegisteredGlyph {
    shape: GlyphShapeId,
    origin_em: Point,
    size_em: Point,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextDraw<'a> {
    pub font: TextFontRef,
    pub run: &'a TextRun,
    pub origin_px: Point,
    pub px_per_sp: f64,
    pub color: LinearRgba,
}

/// Font-routing seam between score text references and configured `DrawText`s.
pub trait ScoreTextBackend {
    fn begin_batch(&mut self, cx: &mut Cx2d, font: TextFontRef);
    fn draw(&mut self, cx: &mut Cx2d, draw: TextDraw<'_>) -> bool;
    fn end_batch(&mut self, cx: &mut Cx2d, font: TextFontRef);
}

/// Adapter for a single app-configured Makepad `DrawText` font family.
pub struct SingleFontTextBackend<'a> {
    pub font: TextFontRef,
    pub draw_text: &'a mut DrawText,
}

impl ScoreTextBackend for SingleFontTextBackend<'_> {
    fn begin_batch(&mut self, cx: &mut Cx2d, font: TextFontRef) {
        if font == self.font {
            self.draw_text.begin_many_instances(cx);
        }
    }

    fn draw(&mut self, cx: &mut Cx2d, draw: TextDraw<'_>) -> bool {
        if draw.font != self.font {
            return false;
        }
        let base_size = self.draw_text.text_style.font_size.max(0.001);
        self.draw_text.font_scale = (draw.run.size * draw.px_per_sp) as f32 / base_size;
        self.draw_text.color = vec4(draw.color.r, draw.color.g, draw.color.b, draw.color.a);
        self.draw_text.draw_abs(
            cx,
            Vec2d {
                x: draw.origin_px.x,
                y: draw.origin_px.y,
            },
            &draw.run.text,
        );
        true
    }

    fn end_batch(&mut self, cx: &mut Cx2d, font: TextFontRef) {
        if font == self.font {
            self.draw_text.end_many_instances(cx);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuDrawOptions {
    /// Zero while zoom moves, then ease to one over 90 ms with
    /// [`crate::SettledZoomTransition`].
    pub snap_progress: f32,
    pub ribbon_error_px: f64,
    pub print_reference: bool,
    /// Static-page alpha used for raster/vector LOD crossfades.
    pub opacity: f32,
    /// Physical pixels per output unit (`Cx::current_dpi_factor`). Hairlines
    /// snap to *this* grid; on a 2x display a logical-point minimum would draw
    /// every staff line and stem at twice the engraved weight.
    pub device_scale: f64,
}

impl Default for GpuDrawOptions {
    fn default() -> Self {
        Self {
            snap_progress: 1.0,
            ribbon_error_px: 0.20,
            print_reference: false,
            opacity: 1.0,
            device_scale: 1.0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GpuDrawStats {
    pub vector_draw_calls: usize,
    pub glyph_instances: usize,
    pub text_runs: usize,
    pub overlay_instances: usize,
    pub missing_glyphs: usize,
    pub missing_text_fonts: usize,
}

/// Makepad replay adapter for vector GPU and `MAKEPAD=headless` targets.
///
/// `DrawGlyph` stores one scale-independent curve/band representation per
/// registered music outline. Visible glyph calls remain adjacent by font and
/// are coalesced by Makepad's instance append path. Analytic primitives are
/// accumulated into one `DrawVector` geometry submission per planned batch;
/// text explicitly uses `begin_many_instances`/`end_many_instances`.
#[derive(Clone, Debug, Default)]
pub struct MakepadScoreRenderer {
    glyphs: BTreeMap<GlyphKey, RegisteredGlyph>,
}

impl MakepadScoreRenderer {
    pub fn register_glyph(
        &mut self,
        draw_glyph: &mut DrawGlyph,
        font: MusicFontRef,
        glyph: SmuflGlyph,
        outline: &GlyphOutline,
    ) -> Option<GlyphShapeId> {
        if outline.units_per_em == 0 || outline.commands.is_empty() {
            return None;
        }
        let key = GlyphKey { font, glyph };
        if let Some(registered) = self.glyphs.get(&key) {
            return Some(registered.shape);
        }
        let scale = 1.0 / outline.units_per_em as f32;
        draw_glyph.begin_shape();
        draw_glyph.set_color(1.0, 1.0, 1.0, 1.0);
        for command in outline.commands.iter() {
            match *command {
                GlyphOutlineCommand::MoveTo(x, y) => draw_glyph.move_to(x * scale, -y * scale),
                GlyphOutlineCommand::LineTo(x, y) => draw_glyph.line_to(x * scale, -y * scale),
                GlyphOutlineCommand::QuadTo(cx, cy, x, y) => {
                    draw_glyph.quad_to(cx * scale, -cy * scale, x * scale, -y * scale)
                }
                GlyphOutlineCommand::CubicTo(c1x, c1y, c2x, c2y, x, y) => draw_glyph.bezier_to(
                    c1x * scale,
                    -c1y * scale,
                    c2x * scale,
                    -c2y * scale,
                    x * scale,
                    -y * scale,
                ),
                GlyphOutlineCommand::Close => draw_glyph.close(),
            }
        }
        draw_glyph.fill_layer();
        let shape = draw_glyph.commit_shape(None)?;
        let metrics = draw_glyph.shape(shape)?;
        let registered = RegisteredGlyph {
            shape,
            origin_em: Point::new(metrics.origin.x as f64, metrics.origin.y as f64),
            size_em: Point::new(metrics.size.x as f64, metrics.size.y as f64),
        };
        self.glyphs.insert(key, registered);
        Some(shape)
    }

    pub fn clear_glyphs(&mut self, draw_glyph: &mut DrawGlyph) {
        self.glyphs.clear();
        draw_glyph.clear_shapes();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        cx: &mut Cx2d,
        plan: &RenderPlan,
        palette: ScorePalette,
        draw_glyph: &mut DrawGlyph,
        draw_vector: &mut DrawVector,
        text_backend: &mut impl ScoreTextBackend,
        options: GpuDrawOptions,
    ) -> GpuDrawStats {
        let mut stats = GpuDrawStats::default();
        let snap_progress = if options.print_reference {
            0.0
        } else {
            options.snap_progress.clamp(0.0, 1.0)
        };
        let opacity = options.opacity.clamp(0.0, 1.0);
        if !plan.pages.is_empty() {
            draw_vector.begin();
            set_vector_color(draw_vector, multiply_alpha(palette.paper, opacity));
            for page in &plan.pages {
                let size = page.page.page_size();
                let rect = page.transform.rect(Rect::from_xywh(0.0, 0.0, size.x, size.y));
                draw_vector.rect(
                    rect.min.x as f32,
                    rect.min.y as f32,
                    rect.width() as f32,
                    rect.height() as f32,
                );
                draw_vector.fill();
            }
            draw_vector.end(cx);
            stats.vector_draw_calls += 1;
        }

        // Every wash goes down before the notation. A selection painted on
        // top of the music is a slab over it, whatever its alpha.
        self.draw_overlays(
            cx,
            plan,
            palette,
            draw_glyph,
            draw_vector,
            text_backend,
            options,
            true,
            &mut stats,
        );

        for batch in &plan.batches {
            match batch.key.pipeline {
                crate::Pipeline::Primitive(_) => {
                    let staff_rules =
                        staff_rule_overrides(plan, batch, snap_progress, options.device_scale);
                    draw_vector.begin();
                    for item_ref in &batch.items {
                        let (item, transform) = resolve_item(plan, *item_ref);
                        let PaintKind::Primitive(primitive) = &item.kind else {
                            continue;
                        };
                        set_vector_color(
                            draw_vector,
                            multiply_alpha(palette.resolve(item.ink), opacity),
                        );
                        if let Some(rect) = staff_rules.get(item_ref) {
                            draw_vector.rect(
                                rect.min.x as f32,
                                rect.min.y as f32,
                                rect.width() as f32,
                                rect.height() as f32,
                            );
                            draw_vector.fill();
                            continue;
                        }
                        append_primitive(
                            draw_vector,
                            primitive,
                            transform,
                            snap_progress,
                            options.ribbon_error_px,
                            options.device_scale,
                            0.0,
                        );
                    }
                    draw_vector.end(cx);
                    stats.vector_draw_calls += 1;
                }
                crate::Pipeline::MusicGlyph(_) => {
                    for item_ref in &batch.items {
                        let (item, transform) = resolve_item(plan, *item_ref);
                        let PaintKind::Glyph(glyph) = &item.kind else {
                            continue;
                        };
                        if self.draw_glyph_item(
                            cx,
                            draw_glyph,
                            glyph,
                            transform,
                            multiply_alpha(palette.resolve(item.ink), opacity),
                            0.0,
                        ) {
                            stats.glyph_instances += 1;
                        } else {
                            stats.missing_glyphs += 1;
                        }
                    }
                }
                crate::Pipeline::Text(font) => {
                    text_backend.begin_batch(cx, font);
                    for item_ref in &batch.items {
                        let (item, transform) = resolve_item(plan, *item_ref);
                        let PaintKind::Text(run) = &item.kind else {
                            continue;
                        };
                        if text_backend.draw(
                            cx,
                            TextDraw {
                                font,
                                run,
                                origin_px: transform.point(run.origin),
                                px_per_sp: transform.scale,
                                color: multiply_alpha(palette.resolve(item.ink), opacity),
                            },
                        ) {
                            stats.text_runs += 1;
                        } else {
                            stats.missing_text_fonts += 1;
                        }
                    }
                    text_backend.end_batch(cx, font);
                }
                crate::Pipeline::Paper => {}
            }
        }
        self.draw_overlays(
            cx,
            plan,
            palette,
            draw_glyph,
            draw_vector,
            text_backend,
            options,
            false,
            &mut stats,
        );
        stats
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_overlays(
        &self,
        cx: &mut Cx2d,
        plan: &RenderPlan,
        palette: ScorePalette,
        draw_glyph: &mut DrawGlyph,
        draw_vector: &mut DrawVector,
        text_backend: &mut impl ScoreTextBackend,
        options: GpuDrawOptions,
        underlay: bool,
        stats: &mut GpuDrawStats,
    ) {
        let snap_progress = if options.print_reference {
            0.0
        } else {
            options.snap_progress.clamp(0.0, 1.0)
        };
        for overlay in plan
            .overlays
            .iter()
            .filter(|overlay| overlay.is_underlay() == underlay)
        {
            match *overlay {
                OverlayCommand::MeasureWash {
                    page_slot,
                    rect_sp,
                    corner_px,
                    opacity,
                } => {
                    let transform = plan.pages[page_slot as usize].transform;
                    let rect = transform.rect(rect_sp);
                    draw_vector.begin();
                    set_vector_color(
                        draw_vector,
                        palette
                            .playback_wash
                            .with_alpha(palette.playback_wash.a * opacity),
                    );
                    draw_vector.rounded_rect(
                        rect.min.x as f32,
                        rect.min.y as f32,
                        rect.width() as f32,
                        rect.height() as f32,
                        corner_px,
                    );
                    draw_vector.fill();
                    draw_vector.end(cx);
                    stats.vector_draw_calls += 1;
                    stats.overlay_instances += 1;
                }
                OverlayCommand::PlaybackCursor {
                    page_slot,
                    x_sp,
                    width_px,
                    span_sp,
                } => {
                    // The cursor marks where the music is, so it spans the
                    // system being played — not the whole sheet of paper.
                    let page = &plan.pages[page_slot as usize];
                    let (top_sp, bottom_sp) =
                        span_sp.unwrap_or((0.0, page.page.page_size().y));
                    let top = page.transform.point(Point::new(x_sp, top_sp));
                    let height = (bottom_sp - top_sp).max(0.0) * page.transform.scale;
                    draw_vector.begin();
                    set_vector_color(draw_vector, palette.playback_cursor);
                    draw_vector.rect(
                        (top.x - width_px as f64 * 0.5) as f32,
                        top.y as f32,
                        width_px,
                        height as f32,
                    );
                    draw_vector.fill();
                    draw_vector.end(cx);
                    stats.vector_draw_calls += 1;
                    stats.overlay_instances += 1;
                }
                OverlayCommand::HighlightSource {
                    source,
                    role,
                    halo_px,
                    ..
                } => {
                    let color = match role {
                        HighlightRole::Selection => palette.selection,
                        HighlightRole::Annotation => palette.annotation,
                        HighlightRole::Hover => palette.hover,
                    };
                    // One copy, dilated. Stacking offset copies multiplies the
                    // wash alpha (1-(1-a)^n) and turns a tint into a slab.
                    let dilate = halo_px.max(0.0) as f64;
                    let (item, transform) = resolve_item(plan, source);
                    match &item.kind {
                        PaintKind::Glyph(glyph) => {
                            if self.draw_glyph_item(
                                cx, draw_glyph, glyph, transform, color, dilate,
                            ) {
                                stats.overlay_instances += 1;
                            }
                        }
                        PaintKind::Primitive(primitive) => {
                            draw_vector.begin();
                            set_vector_color(draw_vector, color);
                            append_primitive(
                                draw_vector,
                                primitive,
                                transform,
                                snap_progress,
                                options.ribbon_error_px,
                                options.device_scale,
                                dilate,
                            );
                            stats.overlay_instances += 1;
                            draw_vector.end(cx);
                            stats.vector_draw_calls += 1;
                        }
                        PaintKind::Text(run) => {
                            text_backend.begin_batch(cx, run.font);
                            if text_backend.draw(
                                cx,
                                TextDraw {
                                    font: run.font,
                                    run,
                                    origin_px: transform.point(run.origin),
                                    px_per_sp: transform.scale,
                                    color,
                                },
                            ) {
                                stats.overlay_instances += 1;
                            }
                            text_backend.end_batch(cx, run.font);
                        }
                    }
                }
            }
        }
    }

    /// `dilate_px` grows the drawn shape outward by that many output pixels on
    /// every side, which is how a wash gets a soft edge without a second copy.
    fn draw_glyph_item(
        &self,
        cx: &mut Cx2d,
        draw_glyph: &mut DrawGlyph,
        glyph: &GlyphItem,
        transform: Transform,
        color: LinearRgba,
        dilate_px: f64,
    ) -> bool {
        let key = GlyphKey {
            font: glyph.font,
            glyph: glyph.glyph.clone(),
        };
        let Some(registered) = self.glyphs.get(&key) else {
            return false;
        };
        let Some(shape) = draw_glyph.shape(registered.shape) else {
            return false;
        };
        let mut layers = shape.layers.clone();
        for layer in &mut layers {
            layer.color = vec4(color.r, color.g, color.b, color.a * layer.color.w);
        }
        let origin = transform.point(glyph.origin);
        let scale = glyph.em_size * transform.scale;
        let dilate = dilate_px.max(0.0);
        draw_glyph.draw_layers_abs(
            cx,
            mp_rect(
                origin.x + registered.origin_em.x * scale - dilate,
                origin.y + registered.origin_em.y * scale - dilate,
                registered.size_em.x * scale + dilate * 2.0,
                registered.size_em.y * scale + dilate * 2.0,
            ),
            &layers,
        );
        true
    }
}

fn resolve_item(plan: &RenderPlan, item_ref: PlannedItemRef) -> (&crate::PaintItem, Transform) {
    let page = &plan.pages[item_ref.page_slot as usize];
    (&page.page.items()[item_ref.paint_index as usize], page.transform)
}

fn set_vector_color(draw: &mut DrawVector, color: LinearRgba) {
    draw.set_color(color.r, color.g, color.b, color.a);
}

fn multiply_alpha(color: LinearRgba, opacity: f32) -> LinearRgba {
    color.with_alpha(color.a * opacity)
}

fn staff_rule_overrides(
    plan: &RenderPlan,
    batch: &InstanceBatch,
    snap_progress: f32,
    device_scale: f64,
) -> BTreeMap<PlannedItemRef, Rect> {
    let mut groups: BTreeMap<(u16, u32), Vec<(PlannedItemRef, Rect)>> = BTreeMap::new();
    for item_ref in &batch.items {
        let (item, _) = resolve_item(plan, *item_ref);
        let PaintKind::Primitive(Primitive::Rule {
            rect,
            kind: RuleKind::Staff,
            staff_group: Some(group),
        }) = &item.kind
        else {
            continue;
        };
        groups
            .entry((item_ref.page_slot, *group))
            .or_default()
            .push((*item_ref, *rect));
    }
    let mut projected = BTreeMap::new();
    for ((page_slot, _), mut group) in groups {
        group.sort_by(|a, b| {
            a.1.center()
                .y
                .total_cmp(&b.1.center().y)
                .then_with(|| a.0.cmp(&b.0))
        });
        let rects: Vec<_> = group.iter().map(|entry| entry.1).collect();
        let transform = plan.pages[page_slot as usize].transform;
        for ((item_ref, _), rule) in group
            .into_iter()
            .zip(project_staff_rules_on_grid(
                &rects,
                transform,
                snap_progress,
                device_scale,
            ))
        {
            projected.insert(item_ref, rule.rect_px);
        }
    }
    projected
}

/// `dilate_px` inflates the emitted geometry by that many output pixels on
/// every side. Zero draws the exact engraved shape; a wash asks for a small
/// positive value so its soft edge reads around the ink it marks.
#[allow(clippy::too_many_arguments)]
fn append_primitive(
    draw: &mut DrawVector,
    primitive: &Primitive,
    transform: Transform,
    snap_progress: f32,
    ribbon_error_px: f64,
    device_scale: f64,
    dilate_px: f64,
) {
    let dilate = dilate_px.max(0.0);
    match primitive {
        Primitive::Rule { rect, .. } => {
            let projected =
                project_rule_on_grid(*rect, transform, snap_progress, device_scale).rect_px;
            draw.rect(
                (projected.min.x - dilate) as f32,
                (projected.min.y - dilate) as f32,
                (projected.width() + dilate * 2.0) as f32,
                (projected.height() + dilate * 2.0) as f32,
            );
            draw.fill();
        }
        Primitive::Beam(beam) => {
            let points = beam.vertices().map(|point| transform.point(point));
            draw.move_to(points[0].x as f32, points[0].y as f32);
            for point in &points[1..] {
                draw.line_to(point.x as f32, point.y as f32);
            }
            draw.close();
            draw.fill();
        }
        Primitive::Ribbon(ribbon) => {
            let mesh = tessellate_ribbon(*ribbon, transform.scale, ribbon_error_px);
            if mesh.vertices.len() < 4 {
                return;
            }
            let first = transform.point(mesh.vertices[0].position);
            draw.move_to(first.x as f32, first.y as f32);
            for pair in mesh.vertices.chunks_exact(2).skip(1) {
                let point = transform.point(pair[0].position);
                draw.line_to(point.x as f32, point.y as f32);
            }
            for pair in mesh.vertices.chunks_exact(2).rev() {
                let point = transform.point(pair[1].position);
                draw.line_to(point.x as f32, point.y as f32);
            }
            draw.close();
            draw.fill();
        }
        Primitive::Hairpin {
            start,
            end,
            opening,
            thickness,
            direction,
        } => {
            let (tip, mouth) = match direction {
                crate::HairpinDirection::Crescendo => (*start, *end),
                crate::HairpinDirection::Diminuendo => (*end, *start),
            };
            let direction = (mouth - tip).normalized();
            let normal = direction.perp() * (*opening * 0.5);
            let tip_gap = normal.normalized() * (*thickness * 0.5);
            for (a, b) in [(tip + tip_gap, mouth + normal), (tip - tip_gap, mouth - normal)] {
                append_line(draw, transform.point(a), transform.point(b));
            }
            draw.stroke((*thickness * transform.scale + dilate * 2.0) as f32);
        }
        Primitive::Bracket {
            x,
            top,
            bottom,
            thickness,
            hook,
        } => {
            let a = transform.point(Point::new(*x + *hook, *top));
            let b = transform.point(Point::new(*x, *top));
            let c = transform.point(Point::new(*x, *bottom));
            let d = transform.point(Point::new(*x + *hook, *bottom));
            draw.move_to(a.x as f32, a.y as f32);
            draw.line_to(b.x as f32, b.y as f32);
            draw.line_to(c.x as f32, c.y as f32);
            draw.line_to(d.x as f32, d.y as f32);
            draw.stroke((*thickness * transform.scale + dilate * 2.0) as f32);
        }
        Primitive::Line {
            start,
            end,
            thickness,
            dash,
            ..
        } => {
            if let Some(pattern) = dash {
                for (start, end) in dash_segments(*start, *end, *pattern) {
                    append_line(draw, transform.point(start), transform.point(end));
                }
            } else {
                append_line(draw, transform.point(*start), transform.point(*end));
            }
            draw.stroke((*thickness * transform.scale + dilate * 2.0) as f32);
        }
        Primitive::TupletBracket {
            start,
            end,
            thickness,
            hook,
            number_gap,
        } => {
            let line = *end - *start;
            let t0 = if line.x.abs() <= f64::EPSILON {
                0.45
            } else {
                ((number_gap.min.x - start.x) / line.x).clamp(0.0, 1.0)
            };
            let t1 = if line.x.abs() <= f64::EPSILON {
                0.55
            } else {
                ((number_gap.max.x - start.x) / line.x).clamp(0.0, 1.0)
            };
            let gap_start = start.lerp(*end, t0.min(t1));
            let gap_end = start.lerp(*end, t0.max(t1));
            let hook_direction = line.normalized().perp() * *hook;
            append_line(draw, transform.point(*start), transform.point(gap_start));
            append_line(draw, transform.point(gap_end), transform.point(*end));
            append_line(
                draw,
                transform.point(*start),
                transform.point(*start + hook_direction),
            );
            append_line(
                draw,
                transform.point(*end),
                transform.point(*end + hook_direction),
            );
            draw.stroke((*thickness * transform.scale + dilate * 2.0) as f32);
        }
    }
}

fn append_line(draw: &mut DrawVector, start: Point, end: Point) {
    draw.move_to(start.x as f32, start.y as f32);
    draw.line_to(end.x as f32, end.y as f32);
}

fn dash_segments(start: Point, end: Point, pattern: DashPattern) -> Vec<(Point, Point)> {
    let delta = end - start;
    let length = delta.length();
    if length <= f64::EPSILON {
        return Vec::new();
    }
    let direction = delta * (1.0 / length);
    let period = pattern.on + pattern.off;
    if period <= f64::EPSILON {
        return vec![(start, end)];
    }
    let mut position = -pattern.phase.rem_euclid(period);
    let mut segments = Vec::new();
    while position < length {
        let on_start = position.max(0.0);
        let on_end = (position + pattern.on).min(length);
        if on_end > on_start {
            segments.push((
                start + direction * on_start,
                start + direction * on_end,
            ));
        }
        position += period;
    }
    segments
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RasterTileRequest {
    pub key: TileKey,
    /// Score-space content bounds, excluding the duplicated AA gutter.
    pub content_sp: Rect,
    pub output_size_px: (u16, u16),
    pub gutter_px: u8,
    /// Use as the sole page transform inside an offscreen Makepad pass.
    pub transform: Transform,
}

/// Produces an offscreen request whose content is replayed through the same
/// renderer. The app owns the render target/texture-array allocation; under
/// `MAKEPAD=headless` the identical pass is software-rasterized and can be PNG
/// encoded by the platform frame writer.
pub fn raster_tile_request(key: TileKey, tile_size_px: u16, gutter_px: u8) -> RasterTileRequest {
    let level = key.level.px_per_sp();
    let content_px = tile_size_px.saturating_sub(gutter_px as u16 * 2).max(1);
    let tile_sp = content_px as f64 / level;
    let content_sp = Rect::from_xywh(
        key.x as f64 * tile_sp,
        key.y as f64 * tile_sp,
        tile_sp,
        tile_sp,
    );
    RasterTileRequest {
        key,
        content_sp,
        output_size_px: (tile_size_px, tile_size_px),
        gutter_px,
        transform: Transform {
            translation: Point::new(
                gutter_px as f64 - content_sp.min.x * level,
                gutter_px as f64 - content_sp.min.y * level,
            ),
            scale: level,
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RasterTileDraw {
    pub key: TileKey,
    pub source_px: Rect,
    pub destination_px: Rect,
    pub alpha: f32,
}

/// App-owned texture-array seam. Resident page tiles can normally be one
/// instanced draw call; texture allocation stays out of the neutral contract.
pub trait ScoreTileBackend {
    fn begin_tiles(&mut self, cx: &mut Cx2d);
    fn draw_tile(&mut self, cx: &mut Cx2d, draw: RasterTileDraw);
    fn end_tiles(&mut self, cx: &mut Cx2d);
}

pub fn replay_raster_tiles(
    cx: &mut Cx2d,
    backend: &mut impl ScoreTileBackend,
    draws: &[RasterTileDraw],
) {
    if draws.is_empty() {
        return;
    }
    backend.begin_tiles(cx);
    for draw in draws {
        backend.draw_tile(cx, *draw);
    }
    backend.end_tiles(cx);
}

pub fn raster_tile_draws(
    lod: &PageLodPlan,
    page_size_sp: Point,
    page_transform: Transform,
    tile_size_px: u16,
    gutter_px: u8,
) -> Vec<RasterTileDraw> {
    if !lod.uses_raster_tiles() {
        return Vec::new();
    }
    let page_bounds = Rect::from_xywh(0.0, 0.0, page_size_sp.x, page_size_sp.y);
    lod.tiles
        .iter()
        .filter_map(|key| {
            let request = raster_tile_request(*key, tile_size_px, gutter_px);
            let content = intersect_rect(request.content_sp, page_bounds)?;
            let level = key.level.px_per_sp();
            Some(RasterTileDraw {
                key: *key,
                source_px: Rect::from_xywh(
                    gutter_px as f64,
                    gutter_px as f64,
                    content.width() * level,
                    content.height() * level,
                ),
                destination_px: page_transform.rect(content),
                alpha: lod.raster_alpha,
            })
        })
        .collect()
}

fn intersect_rect(a: Rect, b: Rect) -> Option<Rect> {
    let min = Point::new(a.min.x.max(b.min.x), a.min.y.max(b.min.y));
    let max = Point::new(a.max.x.min(b.max.x), a.max.y.min(b.max.y));
    (max.x > min.x && max.y > min.y).then_some(Rect { min, max })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PageId, PaletteId, RasterLevel};

    #[test]
    fn dash_phase_is_page_anchored_and_deterministic() {
        let pattern = DashPattern {
            on: 0.5,
            off: 0.25,
            phase: 0.125,
        };
        let a = dash_segments(Point::new(0.0, 0.0), Point::new(2.0, 0.0), pattern);
        let b = dash_segments(Point::new(0.0, 0.0), Point::new(2.0, 0.0), pattern);
        assert_eq!(a, b);
        assert_eq!(a[0], (Point::new(0.0, 0.0), Point::new(0.375, 0.0)));
    }

    #[test]
    fn tile_request_has_exact_gutter_transform() {
        let request = raster_tile_request(
            TileKey {
                page: PageId(2),
                revision: 8,
                palette: PaletteId::Dark,
                level: RasterLevel::TWO,
                x: 1,
                y: 2,
            },
            512,
            2,
        );
        assert_eq!(request.output_size_px, (512, 512));
        assert_eq!(request.content_sp.width(), 254.0);
        assert_eq!(request.transform.point(request.content_sp.min), Point::new(2.0, 2.0));
    }

    #[test]
    fn resident_tile_draws_clip_the_last_tile_to_the_page() {
        let key = TileKey {
            page: PageId(2),
            revision: 8,
            palette: PaletteId::Dark,
            level: RasterLevel::TWO,
            x: 0,
            y: 1,
        };
        let lod = PageLodPlan {
            page: PageId(2),
            level: Some(RasterLevel::TWO),
            raster_alpha: 1.0,
            vector_alpha: 0.0,
            tiles: vec![key],
            missing_tiles: Vec::new(),
        };
        let draws = raster_tile_draws(
            &lod,
            Point::new(180.0, 260.0),
            Transform {
                translation: Point::new(10.0, 20.0),
                scale: 0.5,
            },
            512,
            2,
        );
        assert_eq!(draws.len(), 1);
        assert_eq!(draws[0].destination_px.height(), 3.0);
        assert_eq!(draws[0].source_px.height(), 12.0);
    }
}
