//! The retained score surface. It maps semantic paint pages into screen
//! transforms, asks `score_render` to cull and batch them, and converts real
//! pointer input back into stable semantic IDs.

use crate::{
    action::{AnnotationTool, PageLayout, ScoreAction},
    document::{DragTarget, NoteDrag, SemanticKind, PAGE_HEIGHT_SP, PAGE_WIDTH_SP},
    state::ScoreAppState,
};
use makepad_score::model::AnnotationKind;
use makepad_score_render as render;
use makepad_score_render::{MakepadScoreRenderer, Point as ScorePoint, SemanticId};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.score.*
    use mod.widgets.*

    mod.widgets.ScoreCanvasBase = #(ScoreCanvas::register_widget(vm))
    mod.widgets.ScoreCanvas = set_type_default() do mod.widgets.ScoreCanvasBase{
        width: Fill
        height: Fill
        // The score's own layering rides `draw_depth`, because the renderer
        // coalesces glyph instances into one draw call and paint order alone
        // cannot keep noteheads above staff rules.
        //
        // These stay positive and small. `draw_depth` is written into
        // `world.z`, so a negative depth puts the page behind the near plane
        // and clips the whole score away — a blank sheet of paper. Chrome that
        // must cover the score does NOT compete on depth: the shell puts its
        // dialog and menu layers in their own overlay draw lists, which
        // composite above this one whatever depth it uses.
        draw_bg +: {
            color: score.color_surround
            draw_depth: 0.0
        }
        draw_vector +: {draw_depth: 2.0}
        draw_glyph +: {
            // The AA gutter only enlarges the quad; the vertex shader insets
            // the content rect by the same amount, so it is ink-neutral
            // (measured: identical ink fraction at 1.0 and 3.0). Keep it wide
            // enough for the dilated selection wash's soft edge.
            aa_pad_px: 3.0
            draw_depth: 3.0
        }
        draw_text +: {
            draw_depth: 4.0
            color: score.color_ink_soft
            text_style: theme.font_regular{font_size: 9.0}
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PagePlacement {
    index: usize,
    transform: render::Transform,
}

/// A note being dragged with the pointer down.
///
/// The model snapshot ([`NoteDrag`]) and the resolved landing spot
/// ([`DragTarget`]) live here for the whole gesture: every pointer sample
/// re-resolves the target against the snapshot, and only the drop touches the
/// score — which is what makes one drag exactly one undo step.
#[derive(Clone, Debug)]
struct DragSession {
    drag: NoteDrag,
    target: DragTarget,
    /// Where the pointer went down, in window points.
    origin: DVec2,
    /// Window points per staff space on the dragged note's page.
    scale: f64,
    /// Window points per metrical grid slot of the entry duration.
    slot: f64,
    copy: bool,
    /// True once the drag has asked for a real change; a drag that never does
    /// stays a tap.
    moved: bool,
    auditioned: Option<u8>,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ScoreCanvas {
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
    #[rust]
    area: Area,
    #[rust]
    renderer: MakepadScoreRenderer,
    #[rust]
    placements: Vec<PagePlacement>,
    #[rust]
    glyphs_ready: bool,
    #[rust]
    dragging: bool,
    #[rust]
    ink_points: Vec<ScorePoint>,
    #[rust]
    ink_target: Option<SemanticId>,
    #[rust]
    preview_page: Option<usize>,
    #[rust]
    preview_point: Option<ScorePoint>,
    #[rust]
    last_drag_abs: Option<DVec2>,
    #[rust]
    note_drag: Option<DragSession>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    last_frame_time: Option<f64>,
}

impl ScoreCanvas {
    /// Uploads every glyph of the loaded music font once, so the renderer can
    /// resolve any canonical SMuFL name the engraver emits.
    fn ensure_glyphs(&mut self) {
        if self.glyphs_ready {
            return;
        }
        let font_ref = render::MusicFontRef(0);
        let music_font = crate::font::music_font();
        let mut registered = 0_usize;
        for (name, outline) in music_font.outlines() {
            if self
                .renderer
                .register_glyph(
                    &mut self.draw_glyph,
                    font_ref,
                    render::SmuflGlyph::new(name.to_string()),
                    outline,
                )
                .is_some()
            {
                registered += 1;
            }
        }
        log!("score canvas: registered {registered} music glyphs");
        self.glyphs_ready = true;
    }

    fn rebuild_placements(&mut self, rect: Rect, state: &ScoreAppState) {
        self.placements = page_placements(
            rect,
            state.document.page_count(),
            state.ui.page_layout,
            state.ui.current_page,
            state.ui.zoom,
            state.ui.continuous_scroll,
            state.ui.turn,
        );
    }

    /// Screen point to semantic element.
    ///
    /// Two things matter here. A page that is missing from the document must
    /// only skip *that* placement — bailing out of the whole search made every
    /// later page unhittable. And `PaintList::hit_test` answers in semantic-id
    /// order, which is not proximity order: a bar's full-height hit rect and
    /// the notehead inside it are both hits, so the tightest candidate wins.
    /// Without that, aiming at a note reliably selected the whole bar.
    fn semantic_at(&self, state: &ScoreAppState, abs: DVec2) -> Option<render::SemanticId> {
        for placement in self.placements.iter().rev() {
            let Some(page) = state.document.pages().get(placement.index) else {
                continue;
            };
            let point = render::Point::new(
                (abs.x - placement.transform.translation.x) / placement.transform.scale,
                (abs.y - placement.transform.translation.y) / placement.transform.scale,
            );
            let page_size = page.page_size();
            if point.x < 0.0 || point.y < 0.0 || point.x > page_size.x || point.y > page_size.y {
                continue;
            }
            let tolerance = 2.5 / placement.transform.scale.max(0.1);
            let tightest = tightest_hit(page.hit_test(point, tolerance), |id| {
                state.document.element(id).map(|element| element.bounds)
            });
            if tightest.is_some() {
                return tightest;
            }
        }
        None
    }

    fn page_point_at(&self, abs: DVec2) -> Option<(usize, render::Point)> {
        self.placements.iter().rev().find_map(|placement| {
            let point = render::Point::new(
                (abs.x - placement.transform.translation.x) / placement.transform.scale,
                (abs.y - placement.transform.translation.y) / placement.transform.scale,
            );
            (point.x >= 0.0
                && point.y >= 0.0
                && point.x <= PAGE_WIDTH_SP
                && point.y <= PAGE_HEIGHT_SP)
                .then_some((placement.index, point))
        })
    }

    fn keep_animating(&mut self, cx: &mut Cx) {
        self.next_frame = cx.new_next_frame();
        self.area.redraw(cx);
    }

    /// Opens a drag on a notehead. Only in Editor mode with no annotation tool
    /// armed: elsewhere the pointer means read, not edit.
    fn begin_note_drag(
        &self,
        state: &ScoreAppState,
        semantic: Option<SemanticId>,
        abs: DVec2,
        copy: bool,
    ) -> Option<DragSession> {
        if state.ui.mode != crate::ProductMode::Editor
            || state.ui.annotation_tool != AnnotationTool::None
        {
            return None;
        }
        let drag = state.document.begin_note_drag(semantic?)?;
        let scale = self
            .placements
            .iter()
            .find(|placement| placement.index == drag.page)?
            .transform
            .scale;
        let target =
            state
                .document
                .resolve_note_drag(&drag, 0, 0, state.ui.entry_duration, copy);
        Some(DragSession {
            slot: (drag.slot_width(state.ui.entry_duration) * scale).max(2.0),
            drag,
            target,
            origin: abs,
            scale,
            copy,
            moved: false,
            auditioned: None,
        })
    }

    /// Re-resolves the drag for one pointer sample: vertical travel is
    /// diatonic staff steps (half a staff space each), horizontal travel is
    /// metrical grid slots of the current entry duration.
    fn update_note_drag(&mut self, state: &mut ScoreAppState, abs: DVec2, modifiers: KeyModifiers) {
        let Some(mut session) = self.note_drag.take() else {
            return;
        };
        let delta = abs - session.origin;
        // Half a staff space per step, and up the page is up in pitch.
        let steps = -(delta.y / (0.5 * session.scale)).round() as i32;
        // Shift constrains the drag to pitch alone.
        let slots = if modifiers.shift {
            0
        } else {
            (delta.x / session.slot).round() as i32
        };
        session.copy = modifiers.alt;
        session.target = state.document.resolve_note_drag(
            &session.drag,
            steps,
            slots,
            state.ui.entry_duration,
            session.copy,
        );
        session.moved |= session.target.changes(&session.drag);
        // Audition once per staff step crossed, not once per pointer sample.
        let crossed = session.auditioned != Some(session.target.midi);
        if crossed && session.target.problem.is_none() {
            session.auditioned = Some(session.target.midi);
        }
        if crossed || session.target.problem.is_some() {
            state.preview_note_drag(&session.drag, &session.target, session.copy);
        } else {
            state.ui.status = crate::state::drag_description(
                &state.document,
                &session.drag,
                &session.target,
                session.copy,
            );
        }
        self.note_drag = Some(session);
    }
}

impl Widget for ScoreCanvas {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::NextFrame(frame) = event {
            if frame.set.contains(&self.next_frame) {
                if let Some(state) = scope.data.get_mut::<ScoreAppState>() {
                    let dt = self
                        .last_frame_time
                        .map_or(1.0 / 60.0, |last| (frame.time - last).clamp(0.0, 0.1));
                    self.last_frame_time = Some(frame.time);
                    if state.ui.turn.active {
                        state.ui.turn.progress += dt / 0.24;
                        if state.ui.turn.progress >= 1.0 {
                            state.ui.turn.progress = 1.0;
                            state.ui.turn.active = false;
                        }
                    }
                    state.sync_follow_page();
                    if state.practice.playing || state.ui.turn.active {
                        self.keep_animating(cx);
                    }
                }
                self.area.redraw(cx);
            }
        }

        let Some(state) = scope.data.get_mut::<ScoreAppState>() else {
            return;
        };
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(hover) | Hit::FingerHoverOver(hover) => {
                // The pointer is over the page, not over the strip.
                state.ui.controls_pinned = false;
                state.ui.reveal_controls(Cx::time_now());
                let semantic = self.semantic_at(state, hover.abs);
                let page_point = self.page_point_at(hover.abs);
                self.preview_page = page_point.map(|(page, _)| page);
                self.preview_point = page_point.map(|(_, point)| point);
                state.ui.shadow_pitch = semantic.and_then(|id| {
                    let element = state.document.element(id)?;
                    if element.kind == SemanticKind::Measure {
                        self.preview_point
                            .map(|point| preview_midi(element.bounds, point))
                    } else {
                        None
                    }
                });
                state.audition_semantic(semantic);
                cx.set_cursor(if semantic.is_some() {
                    MouseCursor::Hand
                } else {
                    MouseCursor::Default
                });
                self.area.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                // Leaving the page usually means arriving at the control strip
                // that sits over it, so the controls keep their dwell; only the
                // note-level hover state is dropped here.
                state.release_hover();
                state.ui.shadow_pitch = None;
                self.preview_page = None;
                self.preview_point = None;
                cx.set_cursor(MouseCursor::Default);
                self.area.redraw(cx);
            }
            Hit::FingerDown(down) => {
                cx.set_key_focus(self.area);
                let semantic = self.semantic_at(state, down.abs);
                if down.mouse_button().is_some_and(|button| button.is_secondary()) {
                    cx.action(ScoreAction::ContextMenu {
                        at: down.abs,
                        semantic: semantic.map(|id| id.0),
                    });
                    return;
                }
                self.dragging = true;
                self.last_drag_abs = Some(down.abs);
                if state.ui.annotation_tool == AnnotationTool::Ink {
                    self.ink_target = semantic;
                    self.ink_points.clear();
                    if let Some((_page, point)) = self.page_point_at(down.abs) {
                        self.ink_points.push(point);
                    }
                } else if let Some(session) =
                    self.begin_note_drag(state, semantic, down.abs, down.modifiers.alt)
                {
                    state.handle_canvas_tap(session.drag.semantic, false);
                    self.note_drag = Some(session);
                    self.area.redraw(cx);
                } else if down.modifiers.alt {
                    if let Some(id) = semantic {
                        state.scrub_semantic(id, 1.0);
                    }
                }
            }
            Hit::FingerMove(moved) if self.note_drag.is_some() => {
                self.last_drag_abs = Some(moved.abs);
                self.update_note_drag(state, moved.abs, moved.modifiers);
                cx.set_cursor(MouseCursor::Grabbing);
                self.area.redraw(cx);
            }
            Hit::FingerMove(moved) if self.dragging => {
                let speed = self.last_drag_abs.map_or(1.0, |last| {
                    ((moved.abs - last).length() / 12.0).clamp(0.2, 8.0) as f32
                });
                self.last_drag_abs = Some(moved.abs);
                if state.ui.annotation_tool == AnnotationTool::Ink {
                    if let Some((_page, point)) = self.page_point_at(moved.abs) {
                        self.ink_points.push(point);
                    }
                } else if let Some(id) = self.semantic_at(state, moved.abs) {
                    state.scrub_semantic(id, speed);
                }
                self.area.redraw(cx);
            }
            Hit::FingerUp(up) => {
                if let Some(session) = self.note_drag.take() {
                    self.dragging = false;
                    self.last_drag_abs = None;
                    cx.set_cursor(MouseCursor::Hand);
                    if session.moved {
                        // One gesture, one transaction, one undo step.
                        state.finish_note_drag(
                            cx,
                            &session.drag,
                            &session.target,
                            session.copy,
                        );
                        self.keep_animating(cx);
                        return;
                    }
                    // A drag that never left its note is a plain selection,
                    // which the tap below has already made.
                    state.ui.shadow_pitch = None;
                    self.keep_animating(cx);
                    return;
                }
                let semantic = self.semantic_at(state, up.abs);
                if state.ui.annotation_tool == AnnotationTool::Ink {
                    if let Some(target) = self.ink_target.take() {
                        if self.ink_points.len() >= 2 {
                            let points = std::mem::take(&mut self.ink_points);
                            state.handle_ink(target, &points);
                        }
                    }
                } else if up.was_tap() {
                    if let Some(id) = semantic {
                        let mouse_entry = state
                            .document
                            .element(id)
                            .filter(|element| {
                                state.ui.mode == crate::ProductMode::Editor
                                    && state.ui.annotation_tool == AnnotationTool::None
                                    && element.kind == SemanticKind::Measure
                            })
                            .map(|element| (element.bounds, element.page));
                        if let Some((bounds, page)) = mouse_entry {
                            if let Some((point_page, point)) = self.page_point_at(up.abs) {
                                if point_page == page {
                                    let fraction =
                                        (point.x - bounds.min.x) / bounds.width().max(0.01);
                                    state.handle_mouse_entry(
                                        cx,
                                        id,
                                        preview_midi(bounds, point),
                                        fraction,
                                    );
                                }
                            }
                        } else {
                            state.handle_canvas_tap(id, up.modifiers.shift);
                        }
                    } else {
                        let rect = self.area.rect(cx);
                        let edge = (rect.size.x * 0.16).clamp(42.0, 150.0);
                        if up.abs.x < rect.pos.x + edge {
                            cx.action(ScoreAction::PageDelta(-1));
                        } else if up.abs.x > rect.pos.x + rect.size.x - edge {
                            cx.action(ScoreAction::PageDelta(1));
                        }
                    }
                }
                self.dragging = false;
                self.last_drag_abs = None;
                self.keep_animating(cx);
            }
            Hit::FingerScroll(scroll) => {
                if scroll.modifiers.logo || scroll.modifiers.control {
                    let factor = (1.0 - scroll.scroll.y * 0.002).clamp(0.82, 1.22);
                    cx.action(ScoreAction::ZoomBy(factor));
                } else if state.ui.page_layout == PageLayout::Continuous
                    || state.ui.page_layout == PageLayout::Overview
                {
                    state.ui.continuous_scroll =
                        (state.ui.continuous_scroll + scroll.scroll.y).max(0.0);
                } else if scroll.scroll.x.abs() > 18.0 || scroll.scroll.y.abs() > 42.0 {
                    cx.action(ScoreAction::PageDelta(if scroll.scroll.x + scroll.scroll.y > 0.0 {
                        1
                    } else {
                        -1
                    }));
                }
                self.area.redraw(cx);
            }
            Hit::KeyDown(key) => {
                if let Some(action) = crate::state::key_action(&key, state) {
                    cx.action(action);
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_glyphs();
        // Resolve the walk to a real rect BEFORE drawing: a Fill walk has no
        // size until the turtle is walked, and the vector geometry is built in
        // absolute coordinates from it.
        let rect = cx.walk_turtle(walk);
        self.draw_bg.draw_abs(cx, rect);
        cx.begin_turtle(
            Walk {
                abs_pos: Some(rect.pos),
                width: Size::Fixed(rect.size.x),
                height: Size::Fixed(rect.size.y),
                margin: Inset::default(),
                metrics: Metrics::default(),
            },
            Layout {
                clip_x: true,
                clip_y: true,
                ..Layout::default()
            },
        );

        let Some(state) = scope.data.get_mut::<ScoreAppState>() else {
            cx.end_turtle_with_area(&mut self.area);
            return DrawStep::done();
        };
        self.rebuild_placements(rect, state);

        let views: Vec<_> = self
            .placements
            .iter()
            .filter_map(|placement| {
                state.document.pages().get(placement.index).map(|page| render::PageView {
                    page: page.clone(),
                    transform: placement.transform,
                })
            })
            .collect();
        let (cursor, bar, presentation_quarter) = state.playback_overlay();
        let annotations = state.document.annotation_visuals();
        let overlays = render::OverlayState {
            playback_cursor: cursor,
            playback_bar: bar,
            playback_bar_transition: None,
            presentation_time_s: presentation_quarter,
            selected: state.ui.selection.ordered.clone(),
            annotated: annotations.iter().map(|annotation| annotation.semantic).collect(),
            hovered: state.hovered_sounding(),
        };
        let viewport = render::Rect::from_xywh(rect.pos.x, rect.pos.y, rect.size.x, rect.size.y);
        let plan = render::RenderPlanner.plan(
            &views,
            viewport,
            &overlays,
            render::OverlayMetrics::default(),
        );

        // The policy decision is made for every visible page. Until a tile
        // backend is resident the render crate intentionally promotes the
        // exact vector page; this preserves correctness while still exposing
        // the intended overview LOD boundary.
        let _lod_modes: Vec<_> = plan
            .pages
            .iter()
            .map(|page| render::LodPolicy::default().choose(page.transform.scale))
            .collect();
        let mut text = render::SingleFontTextBackend {
            font: render::TextFontRef(0),
            draw_text: &mut self.draw_text,
        };
        let _stats = self.renderer.draw(
            cx,
            &plan,
            if state.prefs.dark_paper {
                render::ScorePalette::dark()
            } else {
                render::ScorePalette::light()
            },
            &mut self.draw_glyph,
            &mut self.draw_vector,
            &mut text,
            render::GpuDrawOptions {
                // Hairlines snap to the physical pixel grid, not to logical
                // points: a logical-point minimum doubles every staff line and
                // stem on a retina display and blackens a zoomed-out page.
                device_scale: cx.current_dpi_factor(),
                ..render::GpuDrawOptions::default()
            },
        );
        self.draw_annotation_details(cx, state, &annotations);
        self.draw_entry_affordances(cx, state);
        self.draw_note_drag(cx);

        cx.end_turtle_with_area(&mut self.area);
        if state.practice.playing || state.ui.turn.active {
            self.next_frame = cx.new_next_frame();
        }
        DrawStep::done()
    }
}

impl ScoreCanvas {
    fn draw_entry_affordances(&mut self, cx: &mut Cx2d, state: &ScoreAppState) {
        if state.ui.mode != crate::ProductMode::Editor {
            return;
        }
        if let (Some(page), Some(point), Some(_pitch)) = (
            self.preview_page,
            self.preview_point,
            state.ui.shadow_pitch,
        ) {
            if let Some(placement) = self.placements.iter().find(|item| item.index == page) {
                let point = placement.transform.point(point);
                let radius_x = (1.35 * placement.transform.scale).max(2.5) as f32;
                let radius_y = (0.82 * placement.transform.scale).max(1.7) as f32;
                self.draw_vector.begin();
                self.draw_vector.set_color(0.31, 0.62, 0.92, 0.42);
                self.draw_vector
                    .ellipse(point.x as f32, point.y as f32, radius_x, radius_y);
                self.draw_vector.fill();
                self.draw_vector.rect(
                    point.x as f32 + radius_x - 0.8,
                    point.y as f32 - radius_y * 4.2,
                    1.3,
                    radius_y * 4.4,
                );
                self.draw_vector.fill();
                self.draw_vector.end(cx);
            }
        }
        if let Some(element) = state
            .ui
            .caret
            .and_then(|semantic| state.document.element(semantic))
        {
            if let Some(placement) = self
                .placements
                .iter()
                .find(|page| page.index == element.page)
            {
                let bounds = placement.transform.rect(element.bounds);
                self.draw_vector.begin();
                self.draw_vector.set_color(0.18, 0.50, 0.86, 0.95);
                self.draw_vector.rect(
                    bounds.min.x as f32 - 3.0,
                    bounds.min.y as f32 - 7.0,
                    1.5,
                    bounds.height() as f32 + 14.0,
                );
                self.draw_vector.fill();
                self.draw_vector.end(cx);
            }
        }
    }

    /// What a drag looks like while it is happening: the note it started on
    /// stays ringed, a leader runs to where it would land, and the landing
    /// spot carries a ghost notehead on its own staff-step guide — blue while
    /// the drop is legal, red the moment it is not.
    fn draw_note_drag(&mut self, cx: &mut Cx2d) {
        let Some(session) = self.note_drag.clone() else {
            return;
        };
        if !session.moved {
            return;
        }
        let Some(placement) = self
            .placements
            .iter()
            .find(|placement| placement.index == session.target.page)
            .copied()
        else {
            return;
        };
        let scale = placement.transform.scale;
        let from = placement.transform.point(session.drag.origin);
        let to = placement.transform.point(session.target.at);
        let radius_x = (1.35 * scale).max(2.5) as f32;
        let radius_y = (0.82 * scale).max(1.7) as f32;
        let refused = session.target.problem.is_some();
        // The drag has to read *over* the engraving it is moving, so it draws
        // above the glyph and text layers rather than in the vector layer's
        // own slot, where noteheads would cover it.
        let depth = self.draw_vector.draw_depth;
        self.draw_vector.draw_depth = depth + 6.0;
        let (r, g, b) = if refused {
            (0.86, 0.24, 0.20)
        } else if session.copy {
            (0.22, 0.66, 0.42)
        } else {
            (0.18, 0.50, 0.86)
        };

        self.draw_vector.begin();
        // The note the drag started from, still marked.
        self.draw_vector.set_color(r as f32, g as f32, b as f32, 0.30);
        self.draw_vector
            .ellipse(from.x as f32, from.y as f32, radius_x * 1.25, radius_y * 1.45);
        self.draw_vector.stroke(1.2);
        // A leader to the landing spot.
        self.draw_vector.move_to(from.x as f32, from.y as f32);
        self.draw_vector.line_to(to.x as f32, to.y as f32);
        self.draw_vector.stroke(1.0);
        // The staff step it would land on, so the target line is unambiguous.
        let guide = (4.0 * scale).max(8.0) as f32;
        self.draw_vector.set_color(r as f32, g as f32, b as f32, 0.55);
        self.draw_vector.move_to(to.x as f32 - guide, to.y as f32);
        self.draw_vector.line_to(to.x as f32 + guide, to.y as f32);
        self.draw_vector.stroke(1.0);
        // The ghost notehead itself.
        self.draw_vector.set_color(r as f32, g as f32, b as f32, 0.85);
        self.draw_vector
            .ellipse(to.x as f32, to.y as f32, radius_x, radius_y);
        self.draw_vector.fill();
        self.draw_vector.end(cx);
        self.draw_vector.draw_depth = depth;
    }

    fn draw_annotation_details(
        &mut self,
        cx: &mut Cx2d,
        state: &ScoreAppState,
        annotations: &[crate::document::AnnotationVisual],
    ) {
        for annotation in annotations {
            let Some(element) = state.document.element(annotation.semantic) else {
                continue;
            };
            let Some(placement) = self.placements.iter().find(|page| page.index == element.page) else {
                continue;
            };
            let bounds = placement.transform.rect(element.bounds);
            let color = annotation.color;
            match annotation.kind {
                AnnotationKind::Circle => {
                    self.draw_vector.begin();
                    self.draw_vector.set_color(
                        color[0] as f32 / 255.0,
                        color[1] as f32 / 255.0,
                        color[2] as f32 / 255.0,
                        color[3] as f32 / 255.0,
                    );
                    self.draw_vector.ellipse(
                        bounds.center().x as f32,
                        bounds.center().y as f32,
                        bounds.width() as f32 * 0.5 + 5.0,
                        bounds.height() as f32 * 0.5 + 4.0,
                    );
                    self.draw_vector.stroke(2.0);
                    self.draw_vector.end(cx);
                }
                AnnotationKind::Text | AnnotationKind::Fingering => {
                    if let Some(text) = annotation.text.as_deref() {
                        self.draw_text.color = vec4(
                            color[0] as f32 / 255.0,
                            color[1] as f32 / 255.0,
                            color[2] as f32 / 255.0,
                            1.0,
                        );
                        self.draw_text.draw_abs(
                            cx,
                            dvec2(bounds.min.x, bounds.min.y - 13.0),
                            text,
                        );
                    }
                }
                AnnotationKind::Ink => {
                    if annotation.ink_points.len() > 1 {
                        self.draw_vector.begin();
                        self.draw_vector.set_color(
                            color[0] as f32 / 255.0,
                            color[1] as f32 / 255.0,
                            color[2] as f32 / 255.0,
                            color[3] as f32 / 255.0,
                        );
                        let first = placement.transform.point(annotation.ink_points[0]);
                        self.draw_vector.move_to(first.x as f32, first.y as f32);
                        for point in annotation.ink_points.iter().skip(1) {
                            let point = placement.transform.point(*point);
                            self.draw_vector.line_to(point.x as f32, point.y as f32);
                        }
                        self.draw_vector.stroke(2.2);
                        self.draw_vector.end(cx);
                    }
                }
                _ => {}
            }
        }
    }
}

fn page_placements(
    rect: Rect,
    page_count: usize,
    layout: PageLayout,
    current: usize,
    zoom: f64,
    scroll: f64,
    turn: crate::state::PageTurnState,
) -> Vec<PagePlacement> {
    if page_count == 0 || rect.size.x <= 1.0 || rect.size.y <= 1.0 {
        return Vec::new();
    }
    let gap = 18.0;
    let margin = 22.0;
    let current = current.min(page_count - 1);
    let make = |index: usize, x: f64, y: f64, scale: f64| PagePlacement {
        index,
        transform: render::Transform {
            translation: render::Point::new(x, y),
            scale,
        },
    };
    match layout {
        PageLayout::Single => {
            let scale = (((rect.size.x - margin * 2.0) / PAGE_WIDTH_SP)
                .min((rect.size.y - margin * 2.0) / PAGE_HEIGHT_SP)
                * zoom)
                .max(0.05);
            let width = PAGE_WIDTH_SP * scale;
            let height = PAGE_HEIGHT_SP * scale;
            let x = rect.pos.x + (rect.size.x - width) * 0.5;
            let y = rect.pos.y + (rect.size.y - height) * 0.5;
            if turn.active && turn.from < page_count && turn.from != turn.to {
                let smooth = turn.progress * turn.progress * (3.0 - 2.0 * turn.progress);
                let direction = if turn.to > turn.from { 1.0 } else { -1.0 };
                vec![
                    make(turn.from, x - direction * smooth * (width + gap), y, scale),
                    make(turn.to, x + direction * (1.0 - smooth) * (width + gap), y, scale),
                ]
            } else {
                vec![make(current, x, y, scale)]
            }
        }
        PageLayout::TwoUp => {
            let scale = (((rect.size.x - margin * 2.0 - gap) / (PAGE_WIDTH_SP * 2.0))
                .min((rect.size.y - margin * 2.0) / PAGE_HEIGHT_SP)
                * zoom)
                .max(0.05);
            let spread_width = PAGE_WIDTH_SP * scale * 2.0 + gap;
            let x = rect.pos.x + (rect.size.x - spread_width) * 0.5;
            let y = rect.pos.y + (rect.size.y - PAGE_HEIGHT_SP * scale) * 0.5;
            let first = current - current % 2;
            (first..(first + 2).min(page_count))
                .enumerate()
                .map(|(column, index)| make(index, x + column as f64 * (PAGE_WIDTH_SP * scale + gap), y, scale))
                .collect()
        }
        PageLayout::Continuous => {
            let scale = (((rect.size.x - margin * 2.0) / PAGE_WIDTH_SP).min(3.3) * zoom).max(0.05);
            let width = PAGE_WIDTH_SP * scale;
            let x = rect.pos.x + (rect.size.x - width) * 0.5;
            (0..page_count)
                .map(|index| {
                    make(
                        index,
                        x,
                        rect.pos.y + margin + index as f64 * (PAGE_HEIGHT_SP * scale + gap) - scroll,
                        scale,
                    )
                })
                .collect()
        }
        PageLayout::Overview => {
            let aspect = (rect.size.x / rect.size.y.max(1.0)).max(0.3);
            let columns = ((page_count as f64 * aspect * PAGE_HEIGHT_SP / PAGE_WIDTH_SP)
                .sqrt()
                .ceil() as usize)
                .clamp(1, 10);
            let rows = page_count.div_ceil(columns);
            let scale = (((rect.size.x - gap * (columns + 1) as f64) / (PAGE_WIDTH_SP * columns as f64))
                .min((rect.size.y - gap * (rows + 1) as f64) / (PAGE_HEIGHT_SP * rows as f64))
                * zoom)
                .max(0.05);
            let grid_width = columns as f64 * PAGE_WIDTH_SP * scale + (columns - 1) as f64 * gap;
            let x0 = rect.pos.x + (rect.size.x - grid_width) * 0.5;
            (0..page_count)
                .map(|index| {
                    let column = index % columns;
                    let row = index / columns;
                    make(
                        index,
                        x0 + column as f64 * (PAGE_WIDTH_SP * scale + gap),
                        rect.pos.y + gap + row as f64 * (PAGE_HEIGHT_SP * scale + gap) - scroll,
                        scale,
                    )
                })
                .collect()
        }
    }
}

/// Picks the smallest element among the hits.
///
/// `PaintList::hit_test` answers in semantic-id order, which says nothing
/// about proximity: a bar carries a full-height hit rect that contains every
/// notehead in it, so id order decides whether aiming at a note selects the
/// note or the whole bar. Area order always picks the thing the pointer is
/// actually on; ties break on id so the choice stays deterministic.
fn tightest_hit(
    hits: impl IntoIterator<Item = SemanticId>,
    bounds: impl Fn(SemanticId) -> Option<render::Rect>,
) -> Option<SemanticId> {
    hits.into_iter()
        .filter_map(|id| {
            let rect = bounds(id)?;
            Some((id, rect.width().max(0.0) * rect.height().max(0.0)))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
        .map(|(id, _)| id)
}

fn preview_midi(bounds: render::Rect, point: render::Point) -> u8 {
    let relative_y = point.y - bounds.min.y;
    let (staff_middle, middle_midi) = if relative_y < 12.0 {
        (bounds.min.y + 5.0, 71.0)
    } else {
        (bounds.min.y + 19.0, 50.0)
    };
    (middle_midi + (staff_middle - point.y) * 2.0)
        .round()
        .clamp(21.0, 108.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reported symptom was "editing does nothing": aiming at a notehead
    /// kept resolving to the bar's own full-height hit rect, so the note was
    /// never selected and the note-entry path ran instead.
    #[test]
    fn a_notehead_beats_the_bar_rect_that_contains_it() {
        let bar = SemanticId(1);
        let note = SemanticId(900);
        let rects = |id: SemanticId| {
            Some(match id {
                id if id == bar => render::Rect::from_xywh(0.0, 0.0, 40.0, 26.0),
                _ => render::Rect::from_xywh(12.0, 9.0, 1.2, 1.0),
            })
        };
        assert_eq!(tightest_hit([bar, note], rects), Some(note));
        assert_eq!(tightest_hit([note, bar], rects), Some(note));
        assert_eq!(tightest_hit([bar], rects), Some(bar));
        assert_eq!(tightest_hit([], rects), None);
        // An id the document cannot resolve is not a hit at all.
        assert_eq!(tightest_hit([note], |_| None), None);
    }

    #[test]
    fn overview_places_every_page() {
        let placements = page_placements(
            Rect {pos: dvec2(0.0, 0.0), size: dvec2(1200.0, 800.0)},
            24,
            PageLayout::Overview,
            0,
            1.0,
            0.0,
            crate::state::PageTurnState::default(),
        );
        assert_eq!(placements.len(), 24);
        assert!(placements.iter().all(|page| page.transform.scale > 0.0));
    }
}
