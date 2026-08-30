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
use makepad_widgets::{
    scroll_bar::{ScrollAxis, ScrollBarAction},
    *,
};

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
        // The paper's own scrollbars. They hide themselves whenever the whole
        // document fits, so pianist mode at rest never shows one; they appear
        // the moment there is somewhere to go. Their depth is above every
        // score layer (the drag overlay is the highest at 9) because they are
        // chrome drawn over the paper, not ink on it.
        scroll_bar_x: mod.widgets.ScrollBar{
            bar_size: 11.0
            min_handle_size: 34.0
            draw_bg +: {
                draw_depth: 12.0
                size: uniform(5.0)
                color: uniform(score.color_border_light)
                color_hover: uniform(score.color_text_muted)
                color_drag: uniform(score.color_accent)
            }
        }
        scroll_bar_y: mod.widgets.ScrollBar{
            bar_size: 11.0
            min_handle_size: 34.0
            draw_bg +: {
                draw_depth: 12.0
                size: uniform(5.0)
                color: uniform(score.color_border_light)
                color_hover: uniform(score.color_text_muted)
                color_drag: uniform(score.color_accent)
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PagePlacement {
    index: usize,
    transform: render::Transform,
}

/// Gap between pages of the document, in staff spaces.
const PAGE_GAP_SP: f64 = 9.0;
/// The gutter inside a two-up spread: narrower than the gap between spreads,
/// so a spread reads as one opening rather than two loose pages.
const SPREAD_GUTTER_SP: f64 = 2.5;
/// Breathing room kept around the paper, in window points.
const VIEW_MARGIN: f64 = 22.0;
/// Below this many window points per staff space the page is a thumbnail:
/// notation is no longer legible, so a click means "take me there" rather than
/// "select that". A page fits a 960pt-tall window at about 3.8.
const THUMBNAIL_SCALE: f64 = 1.7;
/// How far the pointer must travel before a press becomes a pan. Above the
/// platform's own tap distance (5.0), so a gesture is never both a pan and a
/// tap on the page-turn zones.
const PAN_THRESHOLD: f64 = 6.0;
/// Zoom per point of scroll. A trackpad flick is a handful of points per
/// event, a mouse wheel notch is tens of them, so the per-event factor is
/// clamped to keep one notch from crossing the whole zoom range.
const ZOOM_PER_SCROLL_POINT: f64 = 0.011;
const ZOOM_PER_EVENT: (f64, f64) = (0.75, 1.33);

/// Where every page of the document sits relative to every other, in staff
/// spaces, plus the one scale that maps that space to the window.
///
/// This is the whole geometry model: the document is ONE space, laid out once,
/// and everything that moves the view — grab-pan, both scrollbars, the wheel
/// zoom, a page glide — is a pan and a zoom over it. There is no separate
/// per-page coordinate system to keep in step, which is why panning can cross
/// a page boundary without the view having to change mode.
#[derive(Clone, Debug, Default)]
struct DocLayout {
    /// Page origin in document space, by page index.
    origins: Vec<DVec2>,
    /// The document's own size in staff spaces.
    extent: DVec2,
    /// One page's size in staff spaces.
    page: DVec2,
    /// Window points per staff space.
    scale: f64,
}

impl DocLayout {
    /// The page's rect on screen, given the canvas rect and the view offset.
    fn page_rect(&self, view: Rect, pan: DVec2, index: usize) -> Option<Rect> {
        let origin = *self.origins.get(index)?;
        Some(Rect {
            pos: view.pos + pan + origin * self.scale,
            size: self.page * self.scale,
        })
    }
}

/// A grab-pan in progress: the paper moves with the pointer.
#[derive(Clone, Copy, Debug)]
struct GrabPan {
    origin: DVec2,
    last: DVec2,
    /// True once the pointer has travelled far enough for this to be a pan
    /// rather than a click that happened to wobble.
    active: bool,
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
    #[live]
    scroll_bar_x: ScrollBar,
    #[live]
    scroll_bar_y: ScrollBar,
    #[rust]
    dragging: bool,
    /// The document layout the last frame drew, so pointer handling reasons
    /// about the same geometry the reader is looking at.
    #[rust]
    doc: DocLayout,
    #[rust]
    grab: Option<GrabPan>,
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

    /// Reconcile the view with the state for this frame, in one place: the
    /// requested zoom, the glide towards a page somebody asked for, the pan
    /// clamp, which page the reader is now actually on, and the placements
    /// that follow from all of it.
    ///
    /// Doing it here — once, before anything is drawn or hit-tested — is what
    /// keeps the header, the scrollbars, the transport and the paper from ever
    /// disagreeing about where the reader is.
    fn rebuild_view(&mut self, cx: &mut Cx2d, rect: Rect, state: &mut ScoreAppState) {
        let count = state.document.page_count();
        let mut fit_all = false;
        if state.ui.fit_all {
            state.ui.fit_all = false;
            state.ui.zoom = fit_all_zoom(rect, count, state.ui.page_layout);
            state.ui.glide.active = false;
            fit_all = true;
        }
        let doc = doc_layout(rect, count, state.ui.page_layout, state.ui.zoom);
        if fit_all {
            // "All pages" is about the document, so it centres the document
            // rather than whichever page the reader happened to be on.
            let content = doc.extent * doc.scale;
            state.ui.pan = dvec2(
                (rect.size.x - content.x) * 0.5,
                (rect.size.y - content.y) * 0.5,
            );
            state.ui.recentre = false;
        } else if state.ui.recentre {
            state.ui.recentre = false;
            state.ui.glide.active = false;
            state.ui.pan = centre_pan(rect, &doc, state.ui.current_page);
        } else if state.ui.glide.active {
            // The glide targets a PAGE, so it stays correct even if the window
            // resizes or the zoom moves while it runs.
            let target = centre_pan(rect, &doc, state.ui.glide.page);
            let t = state.ui.glide.progress.clamp(0.0, 1.0);
            let eased = t * t * (3.0 - 2.0 * t);
            state.ui.pan = DVec2::from_lerp(state.ui.glide.from, target, eased);
        }
        state.ui.pan = clamp_pan(state.ui.pan, rect, &doc);
        // What is on screen is what the reader is on. A glide owns the page
        // until it lands, so it cannot be fought by the pages it flies over.
        if !state.ui.glide.active {
            if let Some(page) = page_on_screen(rect, &doc, state.ui.pan) {
                if page != state.ui.current_page {
                    state.ui.current_page = page;
                    // The page indicator, the transport and the library's
                    // current-piece marker are chrome, not paper: they only
                    // repaint when the shell does, so a page reached by
                    // dragging has to ask for that repaint.
                    cx.redraw_all();
                }
            }
        }
        self.placements = placements_for(rect, &doc, state.ui.pan);
        self.doc = doc;
    }

    /// Move the view by hand. Any glide gives way — the hand is the reader
    /// saying where to look, and an animation arguing with it feels broken.
    fn pan_by(&mut self, state: &mut ScoreAppState, delta: DVec2) {
        state.ui.glide.active = false;
        state.ui.pan += delta;
    }

    /// Zoom about a point, keeping the document under that point where it is —
    /// the gesture every map and document viewer has.
    fn zoom_about(&mut self, cx: &mut Cx, state: &mut ScoreAppState, anchor: DVec2, factor: f64) {
        let rect = self.area.rect(cx);
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return;
        }
        let zoom = (state.ui.zoom * factor).clamp(crate::state::ZOOM_MIN, crate::state::ZOOM_MAX);
        if (zoom - state.ui.zoom).abs() < 1e-9 {
            return;
        }
        let count = state.document.page_count();
        let before = doc_layout(rect, count, state.ui.page_layout, state.ui.zoom);
        let after = doc_layout(rect, count, state.ui.page_layout, zoom);
        state.ui.pan = zoom_pan_about(anchor - rect.pos, state.ui.pan, before.scale, after.scale);
        state.ui.zoom = zoom;
        state.ui.glide.active = false;
        state.ui.status = format!("Zoom {}%", (zoom * 100.0).round());
        // The zoom readout and the status line live in the shell.
        cx.redraw_all();
    }

    /// The scrollbars are given the event before the paper is: they are drawn
    /// over it, and a press on a bar must move the view rather than grab the
    /// page behind it. A bar that hits marks the event handled, so the paper's
    /// own hit test below simply does not see it.
    fn handle_scroll_bars(&mut self, cx: &mut Cx, event: &Event, state: &mut ScoreAppState) {
        let mut scrolled_x = None;
        self.scroll_bar_x
            .handle_event_with(cx, event, &mut |_cx, action| {
                if let ScrollBarAction::Scroll { scroll_pos, .. } = action {
                    scrolled_x = Some(scroll_pos);
                }
            });
        let mut scrolled_y = None;
        self.scroll_bar_y
            .handle_event_with(cx, event, &mut |_cx, action| {
                if let ScrollBarAction::Scroll { scroll_pos, .. } = action {
                    scrolled_y = Some(scroll_pos);
                }
            });
        if scrolled_x.is_none() && scrolled_y.is_none() {
            return;
        }
        // Only a bar that actually moved is worth the layout: this runs on
        // every pointer event.
        let rect = self.area.rect(cx);
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return;
        }
        let doc = doc_layout(
            rect,
            state.document.page_count(),
            state.ui.page_layout,
            state.ui.zoom,
        );
        let (_, max) = pan_bounds(rect, &doc);
        if let Some(pos) = scrolled_x {
            state.ui.pan.x = max.x - pos;
        }
        if let Some(pos) = scrolled_y {
            state.ui.pan.y = max.y - pos;
        }
        state.ui.glide.active = false;
        // One more frame after the bar settles: which page the reader is on is
        // derived while the canvas draws, so the header would otherwise show
        // the page they were on before the last drag sample.
        self.keep_animating(cx);
    }

    /// Position and size both bars from the view, then draw them. The thumb is
    /// therefore a readout of the same pan every other gesture writes.
    fn draw_scroll_bars(&mut self, cx: &mut Cx2d, rect: Rect, pan: DVec2) {
        let (_, max) = pan_bounds(rect, &self.doc);
        let total = scroll_total(rect, &self.doc);
        let view = Rect {
            pos: DVec2::default(),
            size: rect.size,
        };
        self.scroll_bar_x.set_scroll_view_total(cx, total.x);
        self.scroll_bar_x.set_scroll_pos_no_action(cx, max.x - pan.x);
        self.scroll_bar_x
            .draw_scroll_bar(cx, ScrollAxis::Horizontal, view, total);
        self.scroll_bar_y.set_scroll_view_total(cx, total.y);
        self.scroll_bar_y.set_scroll_pos_no_action(cx, max.y - pan.y);
        self.scroll_bar_y
            .draw_scroll_bar(cx, ScrollAxis::Vertical, view, total);
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
                    if state.ui.glide.active {
                        state.ui.glide.progress += dt / crate::state::PAGE_GLIDE_S;
                        if state.ui.glide.progress >= 1.0 {
                            state.ui.glide.progress = 1.0;
                            state.ui.glide.active = false;
                        }
                    }
                    state.sync_follow_page();
                    if state.practice.playing || state.ui.glide.active {
                        self.keep_animating(cx);
                    }
                }
                self.area.redraw(cx);
            }
        }

        let Some(state) = scope.data.get_mut::<ScoreAppState>() else {
            return;
        };
        self.handle_scroll_bars(cx, event, state);
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
                // Empty paper is grabbable everywhere, and says so.
                cx.set_cursor(match (semantic.is_some(), self.doc.scale < THUMBNAIL_SCALE) {
                    (_, true) => MouseCursor::Hand,
                    (true, false) => MouseCursor::Hand,
                    (false, false) => MouseCursor::Grab,
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
                } else if down.modifiers.alt && semantic.is_some() {
                    if let Some(id) = semantic {
                        state.scrub_semantic(id, 1.0);
                    }
                } else if semantic.is_none() || self.doc.scale < THUMBNAIL_SCALE {
                    // Nothing under the pointer — or nothing legible to aim at
                    // — so the press takes hold of the paper itself. It only
                    // becomes a pan once it has actually travelled; a press
                    // that does not move is still a click.
                    self.grab = Some(GrabPan {
                        origin: down.abs,
                        last: down.abs,
                        active: false,
                    });
                }
            }
            Hit::FingerMove(moved) if self.note_drag.is_some() => {
                self.last_drag_abs = Some(moved.abs);
                self.update_note_drag(state, moved.abs, moved.modifiers);
                cx.set_cursor(MouseCursor::Grabbing);
                self.area.redraw(cx);
            }
            Hit::FingerMove(moved) if self.grab.is_some() => {
                let Some(mut grab) = self.grab.take() else {
                    return;
                };
                if !grab.active && (moved.abs - grab.origin).length() >= PAN_THRESHOLD {
                    grab.active = true;
                }
                if grab.active {
                    let delta = moved.abs - grab.last;
                    grab.last = moved.abs;
                    self.pan_by(state, delta);
                    cx.set_cursor(MouseCursor::Grabbing);
                    self.area.redraw(cx);
                }
                self.grab = Some(grab);
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
                // A gesture that panned is finished; it was never a click.
                let panned = self.grab.take().is_some_and(|grab| grab.active);
                if panned {
                    self.dragging = false;
                    self.last_drag_abs = None;
                    cx.set_cursor(MouseCursor::Grab);
                    state.ui.status = format!(
                        "Page {} of {}",
                        state.ui.current_page + 1,
                        state.document.page_count()
                    );
                    self.keep_animating(cx);
                    cx.redraw_all();
                    return;
                }
                // Zoomed out, the pages are thumbnails: a click there means
                // "take me to that page", never "edit that note".
                if up.was_tap() && self.doc.scale < THUMBNAIL_SCALE {
                    if let Some((page, _)) = self.page_point_at(up.abs) {
                        state.ui.go_to_page(page);
                        state.ui.zoom = 1.0;
                        state.ui.status = format!(
                            "Page {} of {}",
                            page + 1,
                            state.document.page_count()
                        );
                        self.dragging = false;
                        self.last_drag_abs = None;
                        self.keep_animating(cx);
                        return;
                    }
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
                // Scrolling zooms, about the pointer, the way every map and
                // document viewer works. Pages are reached by dragging the
                // paper, the scrollbars, the click zones, the keys and the
                // transport — a wheel notch is a clumsy way to turn a page and
                // a natural way to change scale. A trackpad pinch would zoom
                // too, but this platform reports no magnify gesture: it
                // arrives as a modified scroll, which lands here as well.
                let factor = (-scroll.scroll.y * ZOOM_PER_SCROLL_POINT)
                    .exp()
                    .clamp(ZOOM_PER_EVENT.0, ZOOM_PER_EVENT.1);
                self.zoom_about(cx, state, scroll.abs, factor);
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
        self.rebuild_view(cx, rect, state);

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
        let pan = state.ui.pan;
        let playing = state.practice.playing;
        let gliding = state.ui.glide.active;
        self.draw_scroll_bars(cx, rect, pan);

        cx.end_turtle_with_area(&mut self.area);
        if playing || gliding {
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

/// Lay the whole document out in staff spaces, and work out how many window
/// points one staff space is worth.
///
/// The layouts differ only in where they put the pages relative to each other
/// and what the zoom is measured against:
///
/// * `Single` is the document as one strip, left to right. Panning sideways
///   walks into the next page; zooming out far enough shows the lot.
/// * `TwoUp` is the same strip in openings, with a narrow gutter inside a
///   spread and the page gap between spreads.
/// * `Continuous` is the strip turned on its side: a column, page above page.
fn doc_layout(rect: Rect, page_count: usize, layout: PageLayout, zoom: f64) -> DocLayout {
    let page = dvec2(PAGE_WIDTH_SP, PAGE_HEIGHT_SP);
    if page_count == 0 || rect.size.x <= 1.0 || rect.size.y <= 1.0 {
        return DocLayout {
            origins: Vec::new(),
            extent: DVec2::default(),
            page,
            scale: 1.0,
        };
    }
    let width = (rect.size.x - VIEW_MARGIN * 2.0).max(1.0);
    let height = (rect.size.y - VIEW_MARGIN * 2.0).max(1.0);
    let (scale, origins) = match layout {
        PageLayout::Single => (
            (width / PAGE_WIDTH_SP).min(height / PAGE_HEIGHT_SP) * zoom,
            (0..page_count)
                .map(|index| dvec2(index as f64 * (PAGE_WIDTH_SP + PAGE_GAP_SP), 0.0))
                .collect(),
        ),
        PageLayout::TwoUp => (
            (width / (PAGE_WIDTH_SP * 2.0 + SPREAD_GUTTER_SP)).min(height / PAGE_HEIGHT_SP) * zoom,
            (0..page_count)
                .map(|index| {
                    let spread = (index / 2) as f64;
                    let side = (index % 2) as f64;
                    dvec2(
                        spread * (PAGE_WIDTH_SP * 2.0 + SPREAD_GUTTER_SP + PAGE_GAP_SP)
                            + side * (PAGE_WIDTH_SP + SPREAD_GUTTER_SP),
                        0.0,
                    )
                })
                .collect(),
        ),
        PageLayout::Continuous => (
            (width / PAGE_WIDTH_SP).min(3.3) * zoom,
            (0..page_count)
                .map(|index| dvec2(0.0, index as f64 * (PAGE_HEIGHT_SP + PAGE_GAP_SP)))
                .collect(),
        ),
    };
    let origins: Vec<DVec2> = origins;
    let extent = origins.iter().fold(DVec2::default(), |extent, origin| {
        dvec2(
            extent.x.max(origin.x + page.x),
            extent.y.max(origin.y + page.y),
        )
    });
    DocLayout {
        origins,
        extent,
        page,
        scale: scale.max(0.02),
    }
}

/// The zoom that brings the entire document into the viewport — the overview,
/// reached by the same zoom control as everything else rather than by a
/// separate mode with its own rules.
fn fit_all_zoom(rect: Rect, page_count: usize, layout: PageLayout) -> f64 {
    let unit = doc_layout(rect, page_count, layout, 1.0);
    if unit.extent.x <= 0.0 || unit.extent.y <= 0.0 {
        return 1.0;
    }
    let width = (rect.size.x - VIEW_MARGIN * 2.0).max(1.0);
    let height = (rect.size.y - VIEW_MARGIN * 2.0).max(1.0);
    let fits = (width / (unit.extent.x * unit.scale)).min(height / (unit.extent.y * unit.scale));
    fits.clamp(crate::state::ZOOM_MIN, crate::state::ZOOM_MAX)
}

/// Padding at the ends of the document, per axis: half the slack around one
/// page. It is what lets the first and last page sit *centred* at the ends of
/// the travel instead of jammed against the edge of the viewport.
fn end_pad(rect: Rect, doc: &DocLayout) -> DVec2 {
    let page = doc.page * doc.scale;
    dvec2(
        ((rect.size.x - page.x) * 0.5).max(0.0),
        ((rect.size.y - page.y) * 0.5).max(0.0),
    )
}

/// How far the view may travel, per axis, as (min, max) offsets.
///
/// When the document (plus its end padding) fits, both bounds are the centred
/// position and the axis is simply locked: there is nowhere to go, and letting
/// the paper be flung into the void is not a feature. When it does not fit, the
/// bounds are exactly "first page centred" and "last page centred", so a drag
/// can cross every page and stops at the true ends of the document.
fn pan_bounds(rect: Rect, doc: &DocLayout) -> (DVec2, DVec2) {
    let content = doc.extent * doc.scale;
    let pad = end_pad(rect, doc);
    let axis = |view: f64, content: f64, pad: f64| {
        if content + pad * 2.0 <= view {
            let centred = (view - content) * 0.5;
            (centred, centred)
        } else {
            (view - content - pad, pad)
        }
    };
    let (min_x, max_x) = axis(rect.size.x, content.x, pad.x);
    let (min_y, max_y) = axis(rect.size.y, content.y, pad.y);
    (dvec2(min_x, min_y), dvec2(max_x, max_y))
}

fn clamp_pan(pan: DVec2, rect: Rect, doc: &DocLayout) -> DVec2 {
    let (min, max) = pan_bounds(rect, doc);
    dvec2(pan.x.clamp(min.x, max.x), pan.y.clamp(min.y, max.y))
}

/// The scrollable extent the bars report: the document plus the end padding,
/// so bar travel and pan travel are the same journey.
fn scroll_total(rect: Rect, doc: &DocLayout) -> DVec2 {
    let content = doc.extent * doc.scale;
    let pad = end_pad(rect, doc);
    dvec2(
        (content.x + pad.x * 2.0).max(rect.size.x),
        (content.y + pad.y * 2.0).max(rect.size.y),
    )
}

/// The view offset that puts one page in the middle of the viewport.
fn centre_pan(rect: Rect, doc: &DocLayout, page: usize) -> DVec2 {
    let Some(origin) = doc.origins.get(page).copied() else {
        return DVec2::default();
    };
    let size = doc.page * doc.scale;
    let pan = dvec2(
        rect.size.x * 0.5 - (origin.x * doc.scale + size.x * 0.5),
        rect.size.y * 0.5 - (origin.y * doc.scale + size.y * 0.5),
    );
    clamp_pan(pan, rect, doc)
}

/// Keep the document point under the pointer under the pointer.
///
/// `anchor` is canvas-local; `from`/`to` are the scales either side of the
/// zoom. Pure, because pointer-centred zoom is the one piece of this that is
/// easy to get subtly wrong and easy to test.
fn zoom_pan_about(anchor: DVec2, pan: DVec2, from: f64, to: f64) -> DVec2 {
    if from <= 0.0 {
        return pan;
    }
    let document_point = (anchor - pan) / from;
    anchor - document_point * to
}

/// The page the reader is looking at: the one showing the most of itself.
fn page_on_screen(rect: Rect, doc: &DocLayout, pan: DVec2) -> Option<usize> {
    let view = Rect {
        pos: DVec2::default(),
        size: rect.size,
    };
    let mut best: Option<(usize, f64)> = None;
    for index in 0..doc.origins.len() {
        let page = doc.page_rect(rect, pan, index)?.translate(-rect.pos);
        let overlap = intersection_area(view, page);
        if overlap <= 0.0 {
            continue;
        }
        if best.is_none_or(|(_, area)| overlap > area) {
            best = Some((index, overlap));
        }
    }
    best.map(|(index, _)| index).or(Some(0))
}

fn intersection_area(a: Rect, b: Rect) -> f64 {
    let x = (a.pos.x + a.size.x).min(b.pos.x + b.size.x) - a.pos.x.max(b.pos.x);
    let y = (a.pos.y + a.size.y).min(b.pos.y + b.size.y) - a.pos.y.max(b.pos.y);
    x.max(0.0) * y.max(0.0)
}

/// The pages worth drawing: everything the viewport touches, plus a page of
/// margin either side so the neighbour a pan is about to reveal is already
/// realised and the crossing does not stutter.
fn placements_for(rect: Rect, doc: &DocLayout, pan: DVec2) -> Vec<PagePlacement> {
    let size = doc.page * doc.scale;
    let prefetch = Rect {
        pos: rect.pos - dvec2(size.x + 1.0, size.y + 1.0),
        size: rect.size + dvec2(size.x, size.y) * 2.0,
    };
    (0..doc.origins.len())
        .filter_map(|index| {
            let page = doc.page_rect(rect, pan, index)?;
            (intersection_area(prefetch, page) > 0.0).then(|| PagePlacement {
                index,
                transform: render::Transform {
                    translation: render::Point::new(page.pos.x, page.pos.y),
                    scale: doc.scale,
                },
            })
        })
        .collect()
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

    fn view(width: f64, height: f64) -> Rect {
        Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(width, height),
        }
    }

    /// The document is one strip: page beside page, in order, with the same
    /// gap between every pair. This is what lets a pan cross a page boundary
    /// without anything changing mode.
    #[test]
    fn pages_lie_left_to_right_as_one_strip() {
        let doc = doc_layout(view(1200.0, 800.0), 6, PageLayout::Single, 1.0);
        assert_eq!(doc.origins.len(), 6);
        let step = doc.origins[1].x - doc.origins[0].x;
        assert!((step - (PAGE_WIDTH_SP + PAGE_GAP_SP)).abs() < 1e-9);
        for pair in doc.origins.windows(2) {
            assert!((pair[1].x - pair[0].x - step).abs() < 1e-9);
            assert_eq!(pair[0].y, 0.0);
        }
        assert!((doc.extent.x - (6.0 * PAGE_WIDTH_SP + 5.0 * PAGE_GAP_SP)).abs() < 1e-9);
        assert_eq!(doc.extent.y, PAGE_HEIGHT_SP);
    }

    /// Continuous is the same strip stood on end, and two-up is the strip in
    /// openings: a narrow gutter inside a spread, the page gap between them.
    #[test]
    fn the_other_layouts_are_the_same_strip_arranged_differently() {
        let column = doc_layout(view(1200.0, 800.0), 4, PageLayout::Continuous, 1.0);
        for pair in column.origins.windows(2) {
            assert_eq!(pair[0].x, 0.0);
            assert!((pair[1].y - pair[0].y - (PAGE_HEIGHT_SP + PAGE_GAP_SP)).abs() < 1e-9);
        }
        let spreads = doc_layout(view(1200.0, 800.0), 4, PageLayout::TwoUp, 1.0);
        let inside = spreads.origins[1].x - spreads.origins[0].x;
        let between = spreads.origins[2].x - spreads.origins[1].x;
        assert!((inside - (PAGE_WIDTH_SP + SPREAD_GUTTER_SP)).abs() < 1e-9);
        assert!(between > inside, "spreads are further apart than the pages inside one");
    }

    /// Panning is bounded by the real ends of the document — the first page
    /// centred at one end, the last page centred at the other — so the paper
    /// can be dragged across every page and never off into empty space.
    #[test]
    fn the_pan_stops_at_the_ends_of_the_document_and_nowhere_between() {
        let rect = view(1200.0, 800.0);
        let doc = doc_layout(rect, 8, PageLayout::Single, 1.0);
        let (min, max) = pan_bounds(rect, &doc);
        assert!(min.x < max.x, "eight pages are wider than the window");
        assert_eq!(clamp_pan(dvec2(10_000.0, 0.0), rect, &doc).x, max.x);
        assert_eq!(clamp_pan(dvec2(-10_000.0, 0.0), rect, &doc).x, min.x);
        // The ends of the travel ARE the first and last page centred.
        assert!((centre_pan(rect, &doc, 0).x - max.x).abs() < 1e-9);
        assert!((centre_pan(rect, &doc, 7).x - min.x).abs() < 1e-9);
        // A page shorter than the window has nowhere to go vertically, so the
        // axis is locked centred rather than free to be flung about.
        assert_eq!(min.y, max.y);
        assert_eq!(clamp_pan(dvec2(0.0, 400.0), rect, &doc).y, min.y);
    }

    /// Zoomed in, the same clamp gives the whole page height back.
    #[test]
    fn zooming_in_unlocks_the_axis_the_page_now_overflows() {
        let rect = view(1200.0, 800.0);
        let doc = doc_layout(rect, 8, PageLayout::Single, 4.0);
        let (min, max) = pan_bounds(rect, &doc);
        assert!(min.y < max.y, "a page taller than the window scrolls vertically");
        let height = doc.page.y * doc.scale;
        assert!((max.y - min.y - (height - rect.size.y)).abs() < 1e-6);
    }

    /// Pointer-centred zoom: whatever is under the pointer stays under it.
    #[test]
    fn zooming_keeps_the_document_under_the_pointer() {
        let anchor = dvec2(300.0, 220.0);
        let pan = dvec2(-140.0, -60.0);
        let (from, to) = (3.8, 5.7);
        let before = (anchor - pan) / from;
        let after_pan = zoom_pan_about(anchor, pan, from, to);
        let after = (anchor - after_pan) / to;
        assert!((after.x - before.x).abs() < 1e-9);
        assert!((after.y - before.y).abs() < 1e-9);
        // Zooming about the same point twice is the same as zooming once.
        let once = zoom_pan_about(anchor, pan, from, to);
        let twice = zoom_pan_about(anchor, zoom_pan_about(anchor, pan, from, 4.5), 4.5, to);
        assert!((once.x - twice.x).abs() < 1e-9);
        assert!((once.y - twice.y).abs() < 1e-9);
    }

    /// The zoom the whole document fits into is inside the range the rest of
    /// the application clamps to, so the overview is reachable by zoom alone.
    #[test]
    fn the_whole_document_fits_inside_the_zoom_range() {
        let rect = view(1200.0, 800.0);
        for count in [1, 4, 24] {
            let zoom = fit_all_zoom(rect, count, PageLayout::Single);
            assert!(zoom > crate::state::ZOOM_MIN && zoom <= crate::state::ZOOM_MAX);
            let doc = doc_layout(rect, count, PageLayout::Single, zoom);
            assert!(doc.extent.x * doc.scale <= rect.size.x + 1.0);
            assert!(doc.extent.y * doc.scale <= rect.size.y + 1.0);
            if count > 1 {
                assert!(doc.scale < THUMBNAIL_SCALE, "a whole document is thumbnails");
            }
        }
        // A document too long to fit even at the smallest legible zoom stops
        // at that zoom rather than shrinking to nothing: what is left is a
        // pannable row of thumbnails, not a grey smear.
        let zoom = fit_all_zoom(rect, 400, PageLayout::Single);
        assert_eq!(zoom, crate::state::ZOOM_MIN);
    }

    /// The reader is on the page that is showing the most of itself, so the
    /// header and the paper cannot disagree once a pan crosses a boundary.
    #[test]
    fn the_current_page_follows_what_is_actually_on_screen() {
        let rect = view(1200.0, 800.0);
        let doc = doc_layout(rect, 5, PageLayout::Single, 1.0);
        for page in 0..5 {
            let pan = centre_pan(rect, &doc, page);
            assert_eq!(page_on_screen(rect, &doc, pan), Some(page));
        }
        // Dragging past the middle of the gap hands the page over.
        let mut pan = centre_pan(rect, &doc, 2);
        let step = (PAGE_WIDTH_SP + PAGE_GAP_SP) * doc.scale;
        pan.x -= step * 0.75;
        assert_eq!(page_on_screen(rect, &doc, pan), Some(3));
    }

    /// Only what the viewport touches is drawn, plus one page of margin so the
    /// neighbour a pan is about to reveal is already realised.
    #[test]
    fn drawing_is_the_visible_pages_and_their_neighbours() {
        let rect = view(1200.0, 800.0);
        let doc = doc_layout(rect, 40, PageLayout::Single, 1.0);
        let placements = placements_for(rect, &doc, centre_pan(rect, &doc, 20));
        assert!(!placements.is_empty());
        assert!(placements.len() <= 6, "40 pages, a handful drawn: {}", placements.len());
        let drawn: Vec<usize> = placements.iter().map(|page| page.index).collect();
        assert!(drawn.contains(&20));
        assert!(drawn.contains(&19) && drawn.contains(&21), "neighbours are prefetched");
        // Every page is reachable: the union over the whole travel is all of them.
        let (min, max) = pan_bounds(rect, &doc);
        let mut seen = std::collections::BTreeSet::new();
        for step in 0..=200 {
            let x = min.x + (max.x - min.x) * step as f64 / 200.0;
            for placement in placements_for(rect, &doc, dvec2(x, max.y)) {
                seen.insert(placement.index);
            }
        }
        assert_eq!(seen.len(), 40);
    }

    /// The scrollbars ride the same offset the hand does: bar travel and pan
    /// travel are one journey, so the thumb can never disagree with the paper.
    #[test]
    fn the_scrollbar_position_is_the_pan_position() {
        let rect = view(1200.0, 800.0);
        let doc = doc_layout(rect, 9, PageLayout::Single, 1.0);
        let (min, max) = pan_bounds(rect, &doc);
        let total = scroll_total(rect, &doc);
        // Pan at the start of the travel is a thumb at the top of the bar, and
        // pan at the end is a thumb at the end of the bar.
        assert!((max.x - max.x).abs() < 1e-9);
        assert!(((max.x - min.x) - (total.x - rect.size.x)).abs() < 1e-6);
        // A bar with nothing to scroll reports no travel at all.
        let single = doc_layout(rect, 1, PageLayout::Single, 1.0);
        let single_total = scroll_total(rect, &single);
        assert!((single_total.x - rect.size.x).abs() < 1e-6);
        assert!((single_total.y - rect.size.y).abs() < 1e-6);
    }

    /// An empty document must not place, clamp or divide by anything.
    #[test]
    fn an_empty_document_has_no_geometry_and_no_panic() {
        let rect = view(1200.0, 800.0);
        let doc = doc_layout(rect, 0, PageLayout::Single, 1.0);
        assert!(doc.origins.is_empty());
        assert!(placements_for(rect, &doc, DVec2::default()).is_empty());
        assert_eq!(page_on_screen(rect, &doc, DVec2::default()), Some(0));
        assert_eq!(centre_pan(rect, &doc, 3), DVec2::default());
        let _ = clamp_pan(dvec2(50.0, 50.0), rect, &doc);
    }
}
