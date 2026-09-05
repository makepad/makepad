//! Lean draw-only score widget.

use crate::{
    document::{DocumentOptions, ScoreDocument, PAGE_HEIGHT_SP, PAGE_WIDTH_SP},
    font,
    spacing::rational_f64,
};
use makepad_score::model::Score;
use makepad_score_render as render;
use makepad_score_render::MakepadScoreRenderer;
use makepad_widgets::{
    scroll_bar::{ScrollAxis, ScrollBarAction},
    *,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ScoreFit = #(ScoreFit::script_api(vm))
    mod.widgets.splat(mod.widgets.ScoreFit)

    mod.widgets.ScoreViewBase = #(ScoreView::register_widget(vm))
    mod.widgets.ScoreView = set_type_default() do mod.widgets.ScoreViewBase {
        width: Fill
        height: Fill
        fit: mod.widgets.ScoreFit.Width
        hide_labels: false
        pan_zoom_gestures: false
        dark: false
        draw_bg +: {
            color: theme.color_bg_app
            draw_depth: 0.0
        }
        draw_vector +: {draw_depth: 2.0}
        draw_glyph +: {
            aa_pad_px: 3.0
            draw_depth: 3.0
        }
        draw_text +: {
            draw_depth: 4.0
            color: theme.color_label_outer
            text_style: theme.font_regular{font_size: 9.0}
        }
        scroll_bar_y: mod.widgets.ScrollBar {
            bar_size: 9.0
            min_handle_size: 28.0
        }
    }
}

const VIEW_MARGIN: f64 = 8.0;
const PAGE_GAP: f64 = 8.0;
const MIN_ZOOM: f64 = 0.25;
const MAX_ZOOM: f64 = 8.0;
const CONTENT_REFLOW_STEPS: usize = 4;
const MAX_CONTENT_PAGE_WIDTH: f64 = PAGE_WIDTH_SP * 4.0;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Script, ScriptHook)]
pub enum ScoreFit {
    #[pick]
    #[default]
    Width,
    Page,
    Content,
}

#[derive(Clone, Copy, Debug)]
struct ContentFitLayout {
    transform: render::Transform,
}

fn content_fit_layout(
    bounds: render::Rect,
    view_size: render::Point,
) -> ContentFitLayout {
    let available_width = (view_size.x - VIEW_MARGIN * 2.0).max(1.0);
    let available_height = (view_size.y - VIEW_MARGIN * 2.0).max(1.0);
    let width_scale = available_width / bounds.width().max(0.01);
    let scale = width_scale
        .min(available_height / bounds.height().max(0.01))
        .max(0.01);
    let translation = render::Point::new(
        (view_size.x - bounds.width() * scale) * 0.5 - bounds.min.x * scale,
        (view_size.y - bounds.height() * scale) * 0.5 - bounds.min.y * scale,
    );
    ContentFitLayout { transform: render::Transform { translation, scale } }
}

fn content_layout_candidate(
    document: &mut ScoreDocument,
    hide_labels: bool,
    page_width: f64,
    view_size: render::Point,
) -> Option<(f64, f64, usize)> {
    // The reflow only runs in Content mode, so the compact-page options
    // (drum key included) are the base; only the page width is the variable.
    let options = DocumentOptions {
        page_size: render::Point::new(page_width, PAGE_HEIGHT_SP),
        ..DocumentOptions::content(document.score(), hide_labels)
    };
    document.set_options(options).ok()?;
    let bounds = document.content_bounds(0)?;
    let available_width = (view_size.x - VIEW_MARGIN * 2.0).max(1.0);
    let available_height = (view_size.y - VIEW_MARGIN * 2.0).max(1.0);
    let scale = (available_width / bounds.width().max(0.01))
        .min(available_height / bounds.height().max(0.01));
    let fill = (bounds.width() * scale / available_width)
        .min(bounds.height() * scale / available_height);
    Some((
        bounds.width() / bounds.height().max(0.01),
        fill,
        document.system_count(0),
    ))
}

/// Reflow content toward the view aspect. The bounds aspect is monotonic in
/// useful page width apart from system-break steps, so keep the best sampled
/// break while four binary iterations close around the target aspect.
fn reflow_content_for_view(
    document: &mut ScoreDocument,
    hide_labels: bool,
    view_size: render::Point,
) -> Option<f64> {
    if view_size.x <= 0.0 || view_size.y <= 0.0 {
        return None;
    }
    let minimum = PAGE_WIDTH_SP * 0.5;
    let widest = document
        .spacing()
        .natural_page_width()
        .clamp(minimum, MAX_CONTENT_PAGE_WIDTH);
    let target_aspect = (view_size.x - VIEW_MARGIN * 2.0).max(1.0)
        / (view_size.y - VIEW_MARGIN * 2.0).max(1.0);
    let mut low = minimum.min(widest);
    let mut high = widest;
    let prefer_single_system = target_aspect >= 2.0;
    let mut best = (false, f64::NEG_INFINITY, high);

    for width in [low, high] {
        if let Some((_, fill, systems)) =
            content_layout_candidate(document, hide_labels, width, view_size)
        {
            let preferred = (systems == 1) == prefer_single_system;
            if (preferred && !best.0) || (preferred == best.0 && fill > best.1) {
                best = (preferred, fill, width);
            }
        }
    }
    for _ in 0..CONTENT_REFLOW_STEPS {
        let width = (low + high) * 0.5;
        let Some((aspect, fill, systems)) =
            content_layout_candidate(document, hide_labels, width, view_size)
        else {
            break;
        };
        let preferred = (systems == 1) == prefer_single_system;
        if (preferred && !best.0) || (preferred == best.0 && fill > best.1) {
            best = (preferred, fill, width);
        }
        if prefer_single_system && systems > 1 {
            low = width;
        } else if !prefer_single_system && systems == 1 {
            high = width;
        } else if aspect < target_aspect {
            low = width;
        } else {
            high = width;
        }
    }
    content_layout_candidate(document, hide_labels, best.2, view_size)?;
    Some(best.2)
}

fn compose_view_transform(
    base: render::Transform,
    viewport: render::Rect,
    zoom: f64,
    offset: DVec2,
) -> render::Transform {
    let center = viewport.center();
    render::Transform {
        translation: render::Point::new(
            center.x + (base.translation.x - center.x) * zoom + offset.x,
            center.y + (base.translation.y - center.y) * zoom + offset.y,
        ),
        scale: base.scale * zoom,
    }
}

fn clamp_pan_offset(
    base: render::Transform,
    bounds: render::Rect,
    viewport: render::Rect,
    zoom: f64,
    offset: DVec2,
) -> DVec2 {
    let unpanned = compose_view_transform(base, viewport, zoom, DVec2::default()).rect(bounds);
    let keep_x = (unpanned.width() * 0.25).min(viewport.width() * 0.25);
    let keep_y = (unpanned.height() * 0.25).min(viewport.height() * 0.25);
    dvec2(
        offset.x.clamp(
            viewport.min.x + keep_x - unpanned.max.x,
            viewport.max.x - keep_x - unpanned.min.x,
        ),
        offset.y.clamp(
            viewport.min.y + keep_y - unpanned.max.y,
            viewport.max.y - keep_y - unpanned.min.y,
        ),
    )
}

fn apply_wheel_zoom(
    base: render::Transform,
    bounds: render::Rect,
    viewport: render::Rect,
    zoom: &mut f64,
    offset: &mut DVec2,
    at: DVec2,
    delta: f64,
) {
    let before = compose_view_transform(base, viewport, *zoom, *offset);
    let score_at = render::Point::new(
        (at.x - before.translation.x) / before.scale,
        (at.y - before.translation.y) / before.scale,
    );
    *zoom = (*zoom * (-delta * 0.0025).exp()).clamp(MIN_ZOOM, MAX_ZOOM);
    let without_offset = compose_view_transform(base, viewport, *zoom, DVec2::default());
    *offset = dvec2(
        at.x - without_offset.translation.x - score_at.x * without_offset.scale,
        at.y - without_offset.translation.y - score_at.y * without_offset.scale,
    );
    *offset = clamp_pan_offset(base, bounds, viewport, *zoom, *offset);
}

fn reset_pan_zoom(zoom: &mut f64, offset: &mut DVec2) {
    *zoom = 1.0;
    *offset = DVec2::default();
}

#[derive(Script, ScriptHook, Widget)]
pub struct ScoreView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_vector: DrawVector,
    #[live]
    draw_glyph: DrawGlyph,
    #[live]
    draw_text: DrawText,
    #[live]
    fit: ScoreFit,
    #[live]
    hide_labels: bool,
    #[live]
    pan_zoom_gestures: bool,
    /// Draw with the designed dark palette (charcoal paper, warm ink) for
    /// hosts whose own surface is dark; the paper itself is `draw_bg`.
    #[live]
    dark: bool,
    #[live]
    scroll_bar_y: ScrollBar,
    #[rust]
    area: Area,
    #[rust]
    document: ScoreDocument,
    #[rust]
    renderer: MakepadScoreRenderer,
    #[rust]
    glyphs_ready: bool,
    #[rust]
    zoom: f64,
    #[rust]
    offset: DVec2,
    #[rust]
    scroll_y: f64,
    #[rust]
    content_height: f64,
    #[rust]
    content_layout_size: DVec2,
    #[rust]
    chosen_page_width: f64,
    #[rust]
    playhead: Option<f64>,
    #[rust]
    grab: Option<(DVec2, DVec2)>,
}

impl ScoreView {
    pub fn set_score(&mut self, cx: &mut Cx, score: Score) {
        font::ensure_default_font();
        self.invalidate_content_layout();
        let options = self.document_options(&score);
        if self.document.set_score_with_options(score, options).is_err() {
            self.document.clear();
        }
        self.reset_navigation();
        self.playhead = None;
        self.redraw(cx);
    }

    pub fn clear(&mut self, cx: &mut Cx) {
        self.document.clear();
        self.invalidate_content_layout();
        self.reset_navigation();
        self.playhead = None;
        self.redraw(cx);
    }

    /// Show a playback cursor at a time measured in whole notes from the
    /// score start. Values outside the score are pinned to its endpoints.
    pub fn set_playhead(&mut self, cx: &mut Cx, at: Option<f64>) {
        let end = self
            .document
            .score()
            .measures
            .values()
            .map(|measure| {
                rational_f64(measure.start.0) + rational_f64(measure.extent.0)
            })
            .max_by(f64::total_cmp);
        let playhead = at.and_then(|at| {
            end.filter(|_| at.is_finite())
                .map(|end| at.clamp(0.0, end))
        });
        if self.playhead != playhead {
            self.playhead = playhead;
            self.redraw(cx);
        }
    }

    pub fn set_zoom(&mut self, cx: &mut Cx, zoom: f64) {
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.redraw(cx);
    }

    pub fn fit_width(&mut self, cx: &mut Cx, on: bool) {
        self.set_fit(cx, if on { ScoreFit::Width } else { ScoreFit::Page });
    }

    pub fn set_fit(&mut self, cx: &mut Cx, fit: ScoreFit) {
        if self.fit == fit {
            return;
        }
        self.fit = fit;
        self.invalidate_content_layout();
        self.rebuild_for_display_options();
        self.reset_navigation();
        self.redraw(cx);
    }

    pub fn set_hide_labels(&mut self, cx: &mut Cx, hide_labels: bool) {
        if self.hide_labels == hide_labels {
            return;
        }
        self.hide_labels = hide_labels;
        self.invalidate_content_layout();
        self.rebuild_for_display_options();
        self.reset_navigation();
        self.redraw(cx);
    }

    pub fn score(&self) -> &Score {
        self.document.score()
    }

    pub fn document(&self) -> &ScoreDocument {
        &self.document
    }

    fn effective_zoom(&self) -> f64 {
        if self.zoom > 0.0 { self.zoom } else { 1.0 }
    }

    fn gestures_enabled(&self) -> bool {
        self.pan_zoom_gestures || self.fit == ScoreFit::Content
    }

    fn invalidate_content_layout(&mut self) {
        self.content_layout_size = DVec2::default();
        self.chosen_page_width = 0.0;
    }

    fn reset_navigation(&mut self) {
        reset_pan_zoom(&mut self.zoom, &mut self.offset);
        self.scroll_y = 0.0;
        self.grab = None;
    }

    fn document_options(&self, score: &Score) -> DocumentOptions {
        if self.fit == ScoreFit::Content {
            DocumentOptions::content(score, self.hide_labels)
        } else {
            DocumentOptions {
                hide_labels: self.hide_labels,
                ..DocumentOptions::default()
            }
        }
    }

    fn rebuild_for_display_options(&mut self) {
        let options = self.document_options(self.document.score());
        if self.document.set_options(options).is_err() {
            self.document.clear();
        }
    }

    fn ensure_content_layout(&mut self, view_size: DVec2) {
        if self.fit != ScoreFit::Content
            || ((view_size.x - self.content_layout_size.x).abs() < 0.5
                && (view_size.y - self.content_layout_size.y).abs() < 0.5)
        {
            return;
        }
        if let Some(width) = reflow_content_for_view(
            &mut self.document,
            self.hide_labels,
            render::Point::new(view_size.x, view_size.y),
        ) {
            self.chosen_page_width = width;
            self.content_layout_size = view_size;
        }
    }

    fn content_bounds(&self) -> render::Rect {
        self.document.content_bounds(0).unwrap_or_else(|| {
            let size = self
                .document
                .pages()
                .first()
                .map(|page| page.page_size())
                .unwrap_or(render::Point::new(1.0, 1.0));
            render::Rect::from_xywh(0.0, 0.0, size.x, size.y)
        })
    }

    fn viewport(rect: Rect) -> render::Rect {
        render::Rect::from_xywh(rect.pos.x, rect.pos.y, rect.size.x, rect.size.y)
    }

    fn fit_transform(&self, rect: Rect) -> render::Transform {
        let page_size = self
            .document
            .pages()
            .first()
            .map(|page| page.page_size())
            .unwrap_or(render::Point::new(1.0, 1.0));
        let (scale, local) = match self.fit {
            ScoreFit::Width => {
                let scale = ((rect.size.x - VIEW_MARGIN * 2.0).max(1.0) / page_size.x)
                    .max(0.01);
                (
                    scale,
                    render::Point::new(
                        (rect.size.x - page_size.x * scale) * 0.5,
                        VIEW_MARGIN,
                    ),
                )
            }
            ScoreFit::Page => {
                let scale = ((rect.size.x - VIEW_MARGIN * 2.0).max(1.0) / page_size.x)
                    .min((rect.size.y - VIEW_MARGIN * 2.0).max(1.0) / page_size.y)
                    .max(0.01);
                let y = if self.document.page_count() <= 1 {
                    (rect.size.y - page_size.y * scale) * 0.5
                } else {
                    VIEW_MARGIN
                };
                (
                    scale,
                    render::Point::new((rect.size.x - page_size.x * scale) * 0.5, y),
                )
            }
            ScoreFit::Content => {
                let layout = content_fit_layout(
                    self.content_bounds(),
                    render::Point::new(rect.size.x, rect.size.y),
                );
                (layout.transform.scale, layout.transform.translation)
            }
        };
        render::Transform {
            translation: render::Point::new(rect.pos.x + local.x, rect.pos.y + local.y),
            scale,
        }
    }

    /// The one composed score-to-view transform. Rendering and overlays must
    /// both use this so fit, cursor-anchored zoom, and pan never diverge.
    fn view_transform(&self, rect: Rect) -> render::Transform {
        let mut transform = compose_view_transform(
            self.fit_transform(rect),
            Self::viewport(rect),
            self.effective_zoom(),
            self.offset,
        );
        if self.fit != ScoreFit::Content {
            transform.translation.y -= self.scroll_y;
        }
        transform
    }

    fn clamp_pan(&mut self, rect: Rect) {
        let scroll = if self.fit == ScoreFit::Content {
            DVec2::default()
        } else {
            dvec2(0.0, -self.scroll_y)
        };
        let combined = clamp_pan_offset(
            self.fit_transform(rect),
            self.content_bounds(),
            Self::viewport(rect),
            self.effective_zoom(),
            self.offset + scroll,
        );
        self.offset = combined - scroll;
    }

    fn apply_wheel_zoom(&mut self, rect: Rect, at: DVec2, delta: f64) {
        let base = self.fit_transform(rect);
        let bounds = self.content_bounds();
        let viewport = Self::viewport(rect);
        let mut zoom = self.effective_zoom();
        let scroll = if self.fit == ScoreFit::Content {
            DVec2::default()
        } else {
            dvec2(0.0, -self.scroll_y)
        };
        let mut combined = self.offset + scroll;
        apply_wheel_zoom(
            base,
            bounds,
            viewport,
            &mut zoom,
            &mut combined,
            at,
            delta,
        );
        self.zoom = zoom;
        self.offset = combined - scroll;
    }

    fn max_scroll(&self, rect: Rect) -> f64 {
        (self.content_height - rect.size.y).max(0.0)
    }

    fn set_scroll(&mut self, cx: &mut Cx, rect: Rect, value: f64) {
        let value = value.clamp(0.0, self.max_scroll(rect));
        if (value - self.scroll_y).abs() > f64::EPSILON {
            self.scroll_y = value;
            self.redraw(cx);
        }
    }

    fn ensure_glyphs(&mut self) {
        if self.glyphs_ready {
            return;
        }
        font::ensure_default_font();
        let font_ref = render::MusicFontRef(0);
        for (name, outline) in font::music_font().outlines() {
            let _ = self.renderer.register_glyph(
                &mut self.draw_glyph,
                font_ref,
                render::SmuflGlyph::new(name.to_string()),
                outline,
            );
        }
        self.glyphs_ready = true;
    }
}

impl Widget for ScoreView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let mut bar_scroll = None;
        if self.fit != ScoreFit::Content {
            self.scroll_bar_y.handle_event_with(cx, event, &mut |_cx, action| {
                if let ScrollBarAction::Scroll { scroll_pos, .. } = action {
                    bar_scroll = Some(scroll_pos);
                }
            });
        }
        let rect = self.area.rect(cx);
        if let Some(scroll) = bar_scroll {
            self.set_scroll(cx, rect, scroll);
        }
        let gestures_enabled = self.gestures_enabled();
        match event.hits(cx, self.area) {
            Hit::FingerScroll(event) => {
                if gestures_enabled {
                    let horizontal_pan = self.effective_zoom() > 1.0 + 1e-9
                        && event.scroll.x.abs() > f64::EPSILON;
                    if horizontal_pan {
                        self.offset.x -= event.scroll.x;
                        self.clamp_pan(rect);
                    }
                    let zoom_delta = if event.scroll.y.abs() > f64::EPSILON {
                        Some(event.scroll.y)
                    } else if !horizontal_pan && event.scroll.x.abs() > f64::EPSILON {
                        Some(event.scroll.x)
                    } else {
                        None
                    };
                    if let Some(delta) = zoom_delta {
                        self.apply_wheel_zoom(rect, event.abs, delta);
                    }
                    self.redraw(cx);
                } else {
                    let delta = if event.scroll.y.abs() > f64::EPSILON {
                        event.scroll.y
                    } else {
                        event.scroll.x
                    };
                    self.set_scroll(cx, rect, self.scroll_y + delta);
                }
            }
            Hit::FingerDown(event) if gestures_enabled && event.is_primary_hit() => {
                if event.tap_count >= 2 {
                    self.reset_navigation();
                    cx.set_cursor(MouseCursor::Grab);
                    self.redraw(cx);
                } else {
                    self.grab = Some((event.abs, self.offset));
                    cx.set_cursor(MouseCursor::Grabbing);
                }
            }
            Hit::FingerMove(event) if gestures_enabled => {
                if let Some((origin, offset)) = self.grab {
                    self.offset = offset + event.abs - origin;
                    self.clamp_pan(rect);
                    self.redraw(cx);
                }
            }
            Hit::FingerUp(_) if gestures_enabled => {
                self.grab = None;
                cx.set_cursor(MouseCursor::Grab);
            }
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) if gestures_enabled => {
                cx.set_cursor(MouseCursor::Grab);
            }
            Hit::FingerHoverOut(_) if gestures_enabled => {
                cx.set_cursor(MouseCursor::Default);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_glyphs();
        let rect = cx.walk_turtle(walk);
        self.ensure_content_layout(rect.size);
        self.draw_bg.draw_abs(cx, rect);
        cx.begin_turtle(
            Walk {
                abs_pos: Some(rect.pos),
                width: Size::Fixed(rect.size.x),
                height: Size::Fixed(rect.size.y),
                ..Default::default()
            },
            Layout { clip_x: true, clip_y: true, ..Layout::default() },
        );

        let first_page_size = self
            .document
            .pages()
            .first()
            .map(|page| page.page_size())
            .unwrap_or(render::Point::new(1.0, 1.0));
        let scale = self.fit_transform(rect).scale * self.effective_zoom();
        self.content_height = if self.fit == ScoreFit::Content {
            rect.size.y
        } else {
            VIEW_MARGIN * 2.0
                + self.document.page_count() as f64 * first_page_size.y * scale
                + self.document.page_count().saturating_sub(1) as f64 * PAGE_GAP
        };
        self.scroll_y = self.scroll_y.clamp(0.0, self.max_scroll(rect));
        if self.gestures_enabled() {
            self.clamp_pan(rect);
        }
        let first_transform = self.view_transform(rect);
        let views = self
            .document
            .pages()
            .iter()
            .enumerate()
            .map(|(index, page)| render::PageView {
                page: page.clone(),
                transform: render::Transform {
                    translation: render::Point::new(
                        first_transform.translation.x,
                        first_transform.translation.y
                            + index as f64 * (first_page_size.y * scale + PAGE_GAP),
                    ),
                    scale: first_transform.scale,
                },
            })
            .collect::<Vec<_>>();
        let viewport = render::Rect::from_xywh(rect.pos.x, rect.pos.y, rect.size.x, rect.size.y);
        let playback_cursor = self.playhead.and_then(|whole| {
            self.document
                .spacing()
                .locate(self.document.score(), whole)
                .map(|location| render::PlaybackPosition {
                    page: render::PageId(location.page as u32),
                    x_sp: location.x_sp,
                    system_span_sp: Some((location.top_sp, location.bottom_sp)),
                })
        });
        let overlays = render::OverlayState {
            playback_cursor,
            ..render::OverlayState::default()
        };
        let plan = render::RenderPlanner.plan(
            &views,
            viewport,
            &overlays,
            render::OverlayMetrics::default(),
        );
        let mut text = render::SingleFontTextBackend {
            font: render::TextFontRef(0),
            draw_text: &mut self.draw_text,
        };
        let _ = self.renderer.draw(
            cx,
            &plan,
            if self.dark { render::ScorePalette::dark() } else { render::ScorePalette::light() },
            &mut self.draw_glyph,
            &mut self.draw_vector,
            &mut text,
            render::GpuDrawOptions {
                device_scale: cx.current_dpi_factor(),
                ..render::GpuDrawOptions::default()
            },
        );

        if self.fit != ScoreFit::Content {
            let view = Rect { pos: DVec2::default(), size: rect.size };
            let total = self.content_height.max(rect.size.y);
            self.scroll_bar_y.set_scroll_view_total(cx, total);
            self.scroll_bar_y.set_scroll_pos_no_action(cx, self.scroll_y);
            self.scroll_bar_y.draw_scroll_bar(
                cx,
                ScrollAxis::Vertical,
                view,
                dvec2(rect.size.x, total),
            );
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

impl ScoreViewRef {
    pub fn set_score(&self, cx: &mut Cx, score: Score) {
        if let Some(mut view) = self.borrow_mut() {
            view.set_score(cx, score);
        }
    }

    pub fn clear(&self, cx: &mut Cx) {
        if let Some(mut view) = self.borrow_mut() {
            view.clear(cx);
        }
    }

    pub fn set_playhead(&self, cx: &mut Cx, at: Option<f64>) {
        if let Some(mut view) = self.borrow_mut() {
            view.set_playhead(cx, at);
        }
    }

    pub fn set_zoom(&self, cx: &mut Cx, zoom: f64) {
        if let Some(mut view) = self.borrow_mut() {
            view.set_zoom(cx, zoom);
        }
    }

    pub fn fit_width(&self, cx: &mut Cx, on: bool) {
        if let Some(mut view) = self.borrow_mut() {
            view.fit_width(cx, on);
        }
    }

    pub fn set_fit(&self, cx: &mut Cx, fit: ScoreFit) {
        if let Some(mut view) = self.borrow_mut() {
            view.set_fit(cx, fit);
        }
    }

    pub fn set_hide_labels(&self, cx: &mut Cx, hide_labels: bool) {
        if let Some(mut view) = self.borrow_mut() {
            view.set_hide_labels(cx, hide_labels);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_drum_score, BuildOptions, DrumHit, DrumVoice};

    #[test]
    fn set_playhead_none_clears_the_stored_cursor() {
        let mut cx = Cx::new(Box::new(|_cx: &mut Cx, _event: &Event| {}));
        let mut view = cx.with_vm(ScoreView::script_new);
        view.playhead = Some(1.0);

        view.set_playhead(&mut cx, None);

        assert_eq!(view.playhead, None);
    }

    fn four_bar_drum_score() -> Score {
        let voices = [
            DrumVoice::Kick,
            DrumVoice::HiHatClosed,
            DrumVoice::Snare,
            DrumVoice::HiHatClosed,
        ];
        let hits: Vec<_> = (0..64)
            .map(|step| DrumHit {
                time_beats: step as f64 * 0.25,
                voice: voices[step % voices.len()],
                velocity: 1.0,
            })
            .collect();
        build_drum_score(
            &hits,
            &BuildOptions { bars: 4, ..BuildOptions::default() },
        )
    }

    fn drum_document() -> ScoreDocument {
        crate::font::ensure_default_font();
        let score = four_bar_drum_score();
        ScoreDocument::with_options(
            score.clone(),
            DocumentOptions::content(&score, true),
        )
        .expect("the drum fixture engraves")
    }

    #[test]
    fn width_remains_the_default_fit() {
        assert_eq!(ScoreFit::default(), ScoreFit::Width);
    }

    #[test]
    fn wide_content_reflows_to_one_system_and_fills_the_view() {
        let mut document = drum_document();
        let view = render::Point::new(1640.0, 400.0);
        let chosen = reflow_content_for_view(&mut document, true, view).unwrap();
        let bounds = document.content_bounds(0).unwrap();
        let layout = content_fit_layout(bounds, view);
        let fitted = layout.transform.rect(bounds);
        assert_eq!(
            document.system_count(0),
            1,
            "chosen page width {chosen}, natural {}",
            document.spacing().natural_page_width()
        );
        assert!(fitted.width() >= view.x * 0.8, "{fitted:?}, width {chosen}");
        assert!(fitted.min.x >= VIEW_MARGIN - 1e-9, "{fitted:?}");
        assert!(fitted.min.y >= VIEW_MARGIN - 1e-9, "{fitted:?}");
        assert!(fitted.max.x <= view.x - VIEW_MARGIN + 1e-9, "{fitted:?}");
        assert!(fitted.max.y <= view.y - VIEW_MARGIN + 1e-9, "{fitted:?}");
    }

    #[test]
    fn square_content_reflows_to_a_stack_and_fills_the_view() {
        let mut document = drum_document();
        let view = render::Point::new(400.0, 400.0);
        let chosen = reflow_content_for_view(&mut document, true, view).unwrap();
        let bounds = document.content_bounds(0).unwrap();
        let layout = content_fit_layout(bounds, view);
        let fitted = layout.transform.rect(bounds);
        assert!(document.system_count(0) >= 2, "chosen page width {chosen}");
        assert!(fitted.height() >= view.y * 0.7, "{fitted:?}, width {chosen}");
        assert!(fitted.min.x >= VIEW_MARGIN - 1e-9, "{fitted:?}");
        assert!(fitted.min.y >= VIEW_MARGIN - 1e-9, "{fitted:?}");
        assert!(fitted.max.x <= view.x - VIEW_MARGIN + 1e-9, "{fitted:?}");
        assert!(fitted.max.y <= view.y - VIEW_MARGIN + 1e-9, "{fitted:?}");
    }

    #[test]
    fn wheel_zoom_keeps_the_score_point_under_the_cursor() {
        let bounds = render::Rect::from_xywh(0.0, 0.0, 300.0, 120.0);
        let viewport = render::Rect::from_xywh(0.0, 0.0, 800.0, 400.0);
        let base = content_fit_layout(bounds, render::Point::new(800.0, 400.0)).transform;
        let at = dvec2(310.0, 175.0);
        let mut zoom = 1.0;
        let mut offset = DVec2::default();
        let before = compose_view_transform(base, viewport, zoom, offset);
        let score_at = render::Point::new(
            (at.x - before.translation.x) / before.scale,
            (at.y - before.translation.y) / before.scale,
        );

        apply_wheel_zoom(
            base,
            bounds,
            viewport,
            &mut zoom,
            &mut offset,
            at,
            -120.0,
        );

        let mapped = compose_view_transform(base, viewport, zoom, offset).point(score_at);
        assert!((mapped.x - at.x).abs() <= 0.5, "{mapped:?} != {at:?}");
        assert!((mapped.y - at.y).abs() <= 0.5, "{mapped:?} != {at:?}");
    }

    #[test]
    fn pan_is_clamped_and_reset_returns_to_fit() {
        let bounds = render::Rect::from_xywh(0.0, 0.0, 300.0, 120.0);
        let viewport = render::Rect::from_xywh(0.0, 0.0, 800.0, 400.0);
        let base = content_fit_layout(bounds, render::Point::new(800.0, 400.0)).transform;
        let zoom = 2.0;
        let offset = clamp_pan_offset(base, bounds, viewport, zoom, dvec2(50_000.0, -50_000.0));
        let visible = compose_view_transform(base, viewport, zoom, offset).rect(bounds);
        let visible_width = visible.max.x.min(viewport.max.x) - visible.min.x.max(viewport.min.x);
        let visible_height = visible.max.y.min(viewport.max.y) - visible.min.y.max(viewport.min.y);
        assert!(visible_width >= viewport.width() * 0.25 - 1e-9, "{visible:?}");
        assert!(visible_height >= viewport.height() * 0.25 - 1e-9, "{visible:?}");

        let mut zoom = zoom;
        let mut offset = offset;
        reset_pan_zoom(&mut zoom, &mut offset);
        assert_eq!(zoom, 1.0);
        assert_eq!(offset, DVec2::default());
    }
}
