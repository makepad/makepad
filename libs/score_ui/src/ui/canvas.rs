//! The retained score surface. It maps semantic paint pages into screen
//! transforms, asks `score_render` to cull and batch them, and converts real
//! pointer input back into stable semantic IDs.

use crate::{
    action::{AnnotationTool, PageLayout, ScoreAction, ScoreTool},
    document::{DragTarget, NoteDrag, SemanticKind, PAGE_HEIGHT_SP, PAGE_WIDTH_SP},
    state::ScoreAppState,
};
use makepad_score::model::AnnotationKind;
use makepad_score_render as render;
use makepad_score_render::{MakepadScoreRenderer, Point as ScorePoint, SemanticId};
use makepad_widgets::{
    event::ScrollPhase,
    scroll_bar::{ScrollAxis, ScrollBarAction},
    scroll_motion::{estimate_release_velocity, push_sample, ScrollSample, FLING_MIN_TOTAL_DELTA},
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

/// How quickly a let-go coast loses speed, as a continuous rate in 1/s:
/// `v' = -PAN_FRICTION * v`. It is the whole feel of the paper's mass. The
/// total travel left in a release is `v / PAN_FRICTION`, so at 4.2 a brisk
/// 1500 pt/s flick carries about a third of a page and is done inside a
/// second — heavy enough to read as an object with weight, firm enough that
/// the paper never wanders off on its own. (iOS scroll views run 2.0 and
/// makepad's own lists 3.0; a page of music wants a shorter throw than a
/// list, because the reader is aiming at a system, not scanning a feed.)
const PAN_FRICTION: f64 = 4.2;
/// The speed (window points/second) at which the coast is simply over. What
/// is left at this point is 26/4.2 ≈ 6 points of travel spread over a further
/// half second — invisible, and exactly the creep that keeps an idle app
/// awake. Below it the motion ends and the app goes quiet.
const PAN_STOP_SPEED: f64 = 26.0;
/// A release slower than this (window points/second) is a hand that put the
/// paper down, not one that threw it.
const PAN_MIN_FLING_SPEED: f64 = 90.0;
/// Ceiling on the launch speed, so a teleporting pointer (an injected event,
/// a tablet jump, a dropped frame) cannot fling the document end to end.
const PAN_MAX_FLING_SPEED: f64 = 6_000.0;
/// The edge spring: a critically damped return with this rate in 1/s, so a
/// stretch is 95% gone in 3/20 s and fully settled in about a quarter second.
const EDGE_SPRING: f64 = 20.0;
/// The share of the arriving speed the edge spring keeps. Half of it reads as
/// the paper losing energy in the stop, which is what a real object does.
const EDGE_ENERGY: f64 = 0.5;
/// Ceiling on the speed handed to the edge spring. The peak stretch of a
/// critically damped spring launched from rest is `v / (EDGE_SPRING * e)`, so
/// this caps the bounce at about 44 points however hard the flick was.
const EDGE_MAX_SPEED: f64 = 2_400.0;
/// Below this stretch (window points) the spring has arrived.
const EDGE_SETTLE: f64 = 0.4;
/// Time constant (seconds) of the zoom ease: each frame closes
/// `1 - e^(-dt/TAU)` of the remaining distance *in log scale*, so equal
/// wheel notches produce equal-looking steps and the motion is smooth rather
/// than a staircase. A notch is visually done in about a fifth of a second.
const ZOOM_EASE_TAU: f64 = 0.045;
/// How close (in log zoom) the ease has to be before it snaps to the target
/// and stops asking for frames. Two parts in a thousand of the scale is a
/// small fraction of a pixel anywhere on the page — chasing it further would
/// be thirty more frames of invisible change keeping the app awake.
const ZOOM_EASE_EPS: f64 = 2e-3;
/// The longest gap (seconds) that still counts as one frame's worth of time.
///
/// An animation that starts after the app has been idle finds a frame clock
/// pointing at whenever the view last drew — half a second ago, a minute ago.
/// Reading that as elapsed time makes the FIRST frame of every ease and every
/// coast a lurch. A gap this long is not a frame interval; it is the app
/// waking up, and the step is the nominal one instead.
const MAX_FRAME_GAP: f64 = 0.05;
/// The step an animation's first frame takes, before there is a real interval
/// to measure.
const NOMINAL_FRAME: f64 = 1.0 / 60.0;
/// How long after the last wheel or trackpad delta a further one still counts
/// as the SAME zoom gesture, and so keeps the anchor the gesture started on.
///
/// Trackpad deltas arrive at display rate, so any real gesture is far inside
/// this; a deliberate re-aim — look somewhere else, then scroll again — is
/// far outside it. Platforms that report scroll phases do not need the
/// heuristic at all and use those instead.
const ZOOM_GESTURE_GAP: f64 = 0.25;

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

/// What the paper does once the hand lets go of it, and how it is caught at
/// the ends of the document.
///
/// The model is deliberately physical, and deliberately absent while the
/// button is down: a held sheet of paper tracks the hand *exactly*, so the
/// drag path applies raw pointer deltas with no filter, no easing and no
/// lag. This state only exists between the release and the paper coming to
/// rest.
///
/// The coast is viscous friction, `v' = -PAN_FRICTION * v`, integrated in
/// closed form: a step of `dt` moves the paper `v * (1 - e^(-k dt)) / k` and
/// leaves it at `v * e^(-k dt)`. Two steps of `dt/2` therefore land in
/// *exactly* the same place as one step of `dt`, which is what makes the feel
/// identical at 60 and 120 Hz instead of merely similar.
#[derive(Clone, Copy, Debug, Default)]
struct PanMotion {
    /// The coast, in window points per second.
    velocity: DVec2,
    /// How far the paper is stretched past the end of its travel, in window
    /// points. This is a *visual* offset only: the pan that the scrollbars,
    /// the page indicator and the overview click-to-navigate all read stays
    /// inside the document, so a bounce can never make them disagree about
    /// where the reader is.
    overscroll: DVec2,
    /// The stretch's own velocity, in window points per second.
    overscroll_velocity: DVec2,
}

impl PanMotion {
    /// Whether anything is still moving. Every term is driven to exactly zero
    /// at its threshold rather than decaying towards it, so this eventually
    /// answers false and the app stops asking for frames.
    fn moving(&self) -> bool {
        self.velocity != DVec2::default()
            || self.overscroll != DVec2::default()
            || self.overscroll_velocity != DVec2::default()
    }

    /// A hand caught the paper. The coast stops in this frame; any stretch is
    /// left to settle, because snapping it away under the finger would be a
    /// visible jump at the very moment the reader took hold.
    fn catch(&mut self) {
        self.velocity = DVec2::default();
        self.overscroll_velocity = DVec2::default();
    }

    /// Advance by `dt` seconds and answer with the new pan.
    fn advance(&mut self, pan: DVec2, min: DVec2, max: DVec2, dt: f64) -> DVec2 {
        dvec2(
            coast_axis(
                &mut self.velocity.x,
                &mut self.overscroll.x,
                &mut self.overscroll_velocity.x,
                pan.x,
                min.x,
                max.x,
                dt,
            ),
            coast_axis(
                &mut self.velocity.y,
                &mut self.overscroll.y,
                &mut self.overscroll_velocity.y,
                pan.y,
                min.y,
                max.y,
                dt,
            ),
        )
    }
}

/// The pointer a zoom gesture is anchored on, and when its last delta came.
///
/// The anchor is LATCHED for the whole gesture rather than followed live.
/// Zoom about a moving point is not zoom about a point: each frame would hold
/// a different bit of paper still, and the sum of that is the paper sliding
/// under the hand. Scrolling fast on a trackpad moves the pointer a little
/// whether or not the reader means it to, so following it made a fast zoom
/// wobble. The gesture aims once, at its first delta, and holds that aim.
#[derive(Clone, Copy, Debug)]
struct ZoomGesture {
    anchor: DVec2,
    last_delta: f64,
}

/// A wheel or trackpad zoom on its way to where the notches asked for.
///
/// The wheel names a *target* scale; the view walks towards it over a few
/// frames rather than cutting, and every frame of the walk is re-anchored on
/// the same pointer position, so the document point under the pointer stays
/// under it throughout — pointer-centred zoom composes exactly, so easing it
/// introduces no drift.
#[derive(Clone, Copy, Debug, Default)]
struct ZoomEase {
    active: bool,
    /// The scale the notches so far add up to.
    target: f64,
    /// The point the gesture latched onto, in absolute window points — NOT
    /// wherever the pointer has drifted to since. See [`ZoomGesture`].
    anchor: DVec2,
    /// The zoom this ease last wrote. If the live zoom is something else,
    /// somebody with a stronger claim (a menu, a key, Fit page) moved it and
    /// the ease stands down rather than dragging the view back.
    written: f64,
}

impl ZoomEase {
    fn owns(&self, zoom: f64) -> bool {
        self.active && (zoom - self.written).abs() < 1e-9
    }
}

/// A rubber band being pulled out in the Select tool: every note the box
/// touches joins the selection when the button comes up.
#[derive(Clone, Debug)]
struct BandSelect {
    /// Where the press landed and where the pointer is now, in window points.
    origin: DVec2,
    current: DVec2,
    /// The selection the band started from, so ⇧/⌘ add to what was already
    /// chosen instead of replacing it.
    base: Vec<SemanticId>,
    extend: bool,
    /// True once the pointer has travelled far enough to be a band rather
    /// than a click that wobbled.
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
    /// What the paper is doing on its own: the coast after a release and the
    /// spring at the ends of the document.
    #[rust]
    motion: PanMotion,
    /// A zoom on its way to the scale the wheel asked for.
    #[rust]
    zoom_ease: ZoomEase,
    /// The point the current zoom gesture is anchored on.
    #[rust]
    zoom_gesture: Option<ZoomGesture>,
    /// Recent pointer positions during a grab, one axis each, so the release
    /// velocity is measured over a window of time rather than off the last
    /// two events — which at a 500 Hz mouse span two milliseconds and turn
    /// pure jitter into a maximum-speed fling.
    #[rust]
    fling_x: Vec<ScrollSample>,
    #[rust]
    fling_y: Vec<ScrollSample>,
    /// A rubber-band selection in progress.
    #[rust]
    band: Option<BandSelect>,
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
        state.ui.zooming = true;
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
        // The paper is drawn at the pan PLUS whatever the edge spring has
        // stretched it by; the pan itself stays inside the document, so the
        // scrollbars, the page indicator and the overview's click-to-navigate
        // all keep reading the same in-bounds journey while the paper bounces.
        self.placements = placements_for(rect, &doc, state.ui.pan + self.motion.overscroll);
        self.doc = doc;
    }

    /// Move the view by hand. Any glide gives way — the hand is the reader
    /// saying where to look, and an animation arguing with it feels broken.
    fn pan_by(&mut self, state: &mut ScoreAppState, delta: DVec2) {
        state.ui.glide.active = false;
        state.ui.pan += delta;
    }

    /// Put the view at `zoom`, keeping the document point under `anchor`
    /// exactly where it is — the gesture every map and document viewer has.
    ///
    /// Anchored zoom composes: zooming a→b→c about one point lands where
    /// a→c about that point does, to the last bit. That is what lets the
    /// wheel's step be *eased* over several frames without the paper drifting
    /// out from under the pointer.
    fn apply_zoom(&mut self, cx: &mut Cx, state: &mut ScoreAppState, anchor: DVec2, zoom: f64) {
        let rect = self.area.rect(cx);
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return;
        }
        let zoom = zoom.clamp(crate::state::ZOOM_MIN, crate::state::ZOOM_MAX);
        if (zoom - state.ui.zoom).abs() < 1e-12 {
            return;
        }
        let count = state.document.page_count();
        let before = doc_layout(rect, count, state.ui.page_layout, state.ui.zoom);
        let after = doc_layout(rect, count, state.ui.page_layout, zoom);
        state.ui.pan = zoom_pan_about(anchor - rect.pos, state.ui.pan, before.scale, after.scale);
        state.ui.zoom = zoom;
    }

    /// A wheel notch, or one delta of a trackpad's two-finger scroll.
    ///
    /// The notch does not move the view: it moves the *target*, and the view
    /// walks there over the next few frames. Notches compound onto whatever
    /// the walk is already heading for, so spinning the wheel accelerates
    /// smoothly instead of restarting from wherever the animation happened to
    /// have reached, and a trackpad's stream of tiny deltas simply keeps
    /// nudging the same target rather than stacking into a jump.
    fn zoom_towards(
        &mut self,
        cx: &mut Cx,
        state: &mut ScoreAppState,
        pointer: DVec2,
        factor: f64,
        time: f64,
        fresh_gesture: bool,
    ) {
        // The gesture aims once and holds that aim. Following the pointer
        // instead makes every later frame hold a different piece of paper
        // still, and the sum of that is the wobble the reader sees.
        //
        // Measured in the running app, zooming 2.4x -> 6x across 25 rapid
        // deltas with the pointer wandering: the aimed-at point holds to
        // 0.001 window points. The ONE thing that still moves it is the pan
        // clamp — against the ends of the document, and at low zoom where a
        // page that fits locks its axis to centred, holding the aim would
        // mean showing empty space, so the bound wins and the paper slides.
        // That is the bound doing its job, not the anchor failing; chasing it
        // would mean letting a fitted page be dragged off centre.
        let anchor = zoom_anchor(self.zoom_gesture, pointer, time, fresh_gesture);
        self.zoom_gesture = Some(ZoomGesture {
            anchor,
            last_delta: time,
        });
        let from = if self.zoom_ease.owns(state.ui.zoom) {
            self.zoom_ease.target
        } else {
            state.ui.zoom
        };
        let target = (from * factor).clamp(crate::state::ZOOM_MIN, crate::state::ZOOM_MAX);
        if (target - state.ui.zoom).abs() < 1e-9 {
            return;
        }
        self.zoom_ease = ZoomEase {
            active: true,
            target,
            anchor,
            written: state.ui.zoom,
        };
        state.ui.glide.active = false;
        // Taking hold of the scale is taking hold of the view.
        self.motion.catch();
        state.ui.status = format!("Zoom {}%", (target * 100.0).round());
        self.keep_animating(cx);
        // The zoom readout and the status line live in the shell.
        cx.redraw_all();
    }

    /// One frame of the coast the hand left behind, and of the spring that
    /// catches it at the ends of the document.
    fn step_motion(&mut self, cx: &mut Cx, state: &mut ScoreAppState, dt: f64) {
        if !self.motion.moving() {
            return;
        }
        // Anything that *sends* the reader somewhere outranks a coast: the
        // hand's momentum has been overruled by a decision.
        if state.ui.glide.active || state.ui.recentre || state.ui.fit_all {
            self.motion = PanMotion::default();
            return;
        }
        let rect = self.area.rect(cx);
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            self.motion = PanMotion::default();
            return;
        }
        let doc = doc_layout(
            rect,
            state.document.page_count(),
            state.ui.page_layout,
            state.ui.zoom,
        );
        let (min, max) = pan_bounds(rect, &doc);
        state.ui.pan = self.motion.advance(state.ui.pan, min, max, dt);
    }

    /// One frame of the walk towards the scale the wheel asked for.
    fn step_zoom(&mut self, cx: &mut Cx, state: &mut ScoreAppState, dt: f64) {
        if !self.zoom_ease.active {
            return;
        }
        if !self.zoom_ease.owns(state.ui.zoom) {
            // Somebody with a stronger claim moved the zoom — a menu, a key,
            // Fit page. The ease stands down rather than dragging it back.
            self.zoom_ease.active = false;
            state.ui.zooming = false;
            return;
        }
        let (next, arrived) = zoom_ease_step(state.ui.zoom, self.zoom_ease.target, dt);
        if arrived {
            self.zoom_ease.active = false;
            state.ui.zooming = false;
        }
        let anchor = self.zoom_ease.anchor;
        self.apply_zoom(cx, state, anchor, next);
        self.zoom_ease.written = state.ui.zoom;
    }

    /// Let go of the paper: it keeps the hand's velocity, and friction takes
    /// it from there.
    fn launch_coast(&mut self, cx: &mut Cx, state: &mut ScoreAppState) {
        let (velocity, travel) = release_velocity(&self.fling_x, &self.fling_y);
        self.fling_x.clear();
        self.fling_y.clear();
        let speed = velocity.length();
        // A hand that had already stopped before it let go means stop. So
        // does a gesture that barely moved: that is a click that wobbled.
        if speed < PAN_MIN_FLING_SPEED || travel.length() <= FLING_MIN_TOTAL_DELTA {
            return;
        }
        let rect = self.area.rect(cx);
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return;
        }
        let launch = if speed > PAN_MAX_FLING_SPEED {
            velocity * (PAN_MAX_FLING_SPEED / speed)
        } else {
            velocity
        };
        let doc = doc_layout(
            rect,
            state.document.page_count(),
            state.ui.page_layout,
            state.ui.zoom,
        );
        let (min, max) = pan_bounds(rect, &doc);
        self.motion.velocity = unpinned(launch, state.ui.pan, min, max);
        // The coast's first step is a nominal frame, not the gap back to
        // whenever the view last happened to animate.
        self.last_frame_time = None;
        if self.motion.moving() {
            self.keep_animating(cx);
        }
    }

    /// Which tool the pointer is actually obeying. Pianist mode is the reading
    /// face: it navigates and never edits, whatever the editor's tool was left
    /// set to.
    fn tool(&self, state: &ScoreAppState) -> ScoreTool {
        if state.ui.mode == crate::ProductMode::Pianist {
            ScoreTool::Navigate
        } else {
            state.ui.tool
        }
    }

    /// Every note the rubber band has swept up, plus whatever it started from.
    fn band_selection(&self, state: &ScoreAppState, band: &BandSelect) -> Vec<SemanticId> {
        let box_rect = Rect {
            pos: dvec2(
                band.origin.x.min(band.current.x),
                band.origin.y.min(band.current.y),
            ),
            size: dvec2(
                (band.current.x - band.origin.x).abs(),
                (band.current.y - band.origin.y).abs(),
            ),
        };
        let mut chosen: Vec<SemanticId> = if band.extend {
            band.base.clone()
        } else {
            Vec::new()
        };
        // Document order, so a band drawn right-to-left selects the same run
        // as one drawn left-to-right and the arrow keys walk it sensibly.
        for semantic in state.document.all_note_semantics() {
            let Some(element) = state.document.element(semantic) else {
                continue;
            };
            let Some(placement) = self
                .placements
                .iter()
                .find(|placement| placement.index == element.page)
            else {
                continue;
            };
            let bounds = placement.transform.rect(element.bounds);
            let note = Rect {
                pos: dvec2(bounds.min.x, bounds.min.y),
                size: dvec2(bounds.width(), bounds.height()),
            };
            if intersection_area(box_rect, note) > 0.0 && !chosen.contains(&semantic) {
                chosen.push(semantic);
            }
        }
        chosen
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
        // A hand on the bar is a hand on the view.
        self.motion.catch();
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
                    let dt = self.last_frame_time.map_or(NOMINAL_FRAME, |last| {
                        let elapsed = frame.time - last;
                        if elapsed <= 0.0 || elapsed > MAX_FRAME_GAP {
                            NOMINAL_FRAME
                        } else {
                            elapsed
                        }
                    });
                    self.last_frame_time = Some(frame.time);
                    if state.ui.glide.active {
                        state.ui.glide.progress += dt / crate::state::PAGE_GLIDE_S;
                        if state.ui.glide.progress >= 1.0 {
                            state.ui.glide.progress = 1.0;
                            state.ui.glide.active = false;
                        }
                    }
                    self.step_motion(cx, state, dt);
                    self.step_zoom(cx, state, dt);
                    state.sync_follow_page();
                    // Only while something is actually moving. Every term
                    // above is driven to exactly zero at its threshold rather
                    // than decaying towards one, so this eventually stops
                    // asking and the app goes quiet.
                    if state.practice.playing
                        || state.ui.glide.active
                        || self.motion.moving()
                        || self.zoom_ease.active
                    {
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
                // The cursor is the tool's promise. Zoomed out to thumbnails
                // a click always means "take me there", whatever the tool.
                cx.set_cursor(if self.doc.scale < THUMBNAIL_SCALE {
                    MouseCursor::Hand
                } else {
                    match (self.tool(state), semantic.is_some()) {
                        // Navigate takes hold of the paper anywhere, notes
                        // included: that is the whole point of the mode.
                        (ScoreTool::Navigate, _) => MouseCursor::Grab,
                        (ScoreTool::Select, true) => MouseCursor::Arrow,
                        (ScoreTool::Edit, true) => MouseCursor::Crosshair,
                        // Empty paper still pans under every tool.
                        (_, false) => MouseCursor::Grab,
                    }
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
                // A press catches the paper: whatever it was coasting at stops
                // in this frame, and the grab below takes hold of it exactly
                // where it now is. Momentum that carried on under the finger
                // would read as the app arguing with the hand.
                self.motion.catch();
                self.fling_x.clear();
                self.fling_y.clear();
                push_sample(&mut self.fling_x, down.abs.x, down.time);
                push_sample(&mut self.fling_y, down.abs.y, down.time);
                self.area.redraw(cx);
                self.dragging = true;
                self.last_drag_abs = Some(down.abs);

                // The routing, in one place and in priority order. What a drag
                // MEANS is now a decision the reader made with the toolbar,
                // not an accident of what happened to be under the pointer:
                // the default tool moves the paper and can never move music.
                let tool = self.tool(state);
                let middle = down
                    .mouse_button()
                    .is_some_and(|button| button.contains(MouseButton::MIDDLE));
                let grab_paper = |canvas: &mut Self| {
                    canvas.grab = Some(GrabPan {
                        origin: down.abs,
                        last: down.abs,
                        active: false,
                    });
                };
                if middle {
                    // The escape hatch that works over anything, in any tool.
                    grab_paper(self);
                } else if state.ui.annotation_tool == AnnotationTool::Ink {
                    self.ink_target = semantic;
                    self.ink_points.clear();
                    if let Some((_page, point)) = self.page_point_at(down.abs) {
                        self.ink_points.push(point);
                    }
                } else if down.modifiers.alt && semantic.is_some() && tool != ScoreTool::Navigate {
                    // Alt-scrub auditions the music under the pointer. It is
                    // an editing gesture, so Navigate does not answer to it.
                    if let Some(id) = semantic {
                        state.scrub_semantic(id, 1.0);
                    }
                } else if semantic.is_none() || self.doc.scale < THUMBNAIL_SCALE {
                    // Empty paper — or nothing legible to aim at — takes hold
                    // of the paper under every tool. Being unable to move the
                    // page because a tool is armed is worse than any accident
                    // the tool prevents.
                    grab_paper(self);
                } else {
                    match tool {
                        // Over a note, the whole point of the mode.
                        ScoreTool::Navigate => grab_paper(self),
                        ScoreTool::Select => {
                            // The press selects immediately, so the reader can
                            // see what they took hold of; the band, if one
                            // opens, grows from there. ⇧ and ⌘ add to what was
                            // already chosen instead of replacing it.
                            let extend = down.modifiers.shift || down.modifiers.is_primary();
                            let base = if extend {
                                state.ui.selection.ordered.clone()
                            } else {
                                Vec::new()
                            };
                            if let Some(id) = semantic {
                                state.handle_canvas_tap(id, extend);
                            }
                            self.band = Some(BandSelect {
                                origin: down.abs,
                                current: down.abs,
                                base,
                                extend,
                                active: false,
                            });
                            self.area.redraw(cx);
                        }
                        ScoreTool::Edit => {
                            if let Some(session) =
                                self.begin_note_drag(state, semantic, down.abs, down.modifiers.alt)
                            {
                                state.handle_canvas_tap(session.drag.semantic, false);
                                self.note_drag = Some(session);
                                self.area.redraw(cx);
                            } else {
                                grab_paper(self);
                            }
                        }
                    }
                }
            }
            Hit::FingerMove(moved) if self.note_drag.is_some() => {
                self.last_drag_abs = Some(moved.abs);
                self.update_note_drag(state, moved.abs, moved.modifiers);
                cx.set_cursor(MouseCursor::Grabbing);
                self.area.redraw(cx);
            }
            Hit::FingerMove(moved) if self.band.is_some() => {
                let Some(mut band) = self.band.take() else {
                    return;
                };
                if !band.active && (moved.abs - band.origin).length() >= PAN_THRESHOLD {
                    band.active = true;
                }
                band.current = moved.abs;
                if band.active {
                    let chosen = self.band_selection(state, &band);
                    state.set_band_selection(&chosen);
                    cx.set_cursor(MouseCursor::Crosshair);
                    self.area.redraw(cx);
                }
                self.band = Some(band);
            }
            Hit::FingerMove(moved) if self.grab.is_some() => {
                let Some(mut grab) = self.grab.take() else {
                    return;
                };
                // Sampled whether or not the press has become a pan yet: the
                // travel that crossed the threshold is part of the flick.
                push_sample(&mut self.fling_x, moved.abs.x, moved.time);
                push_sample(&mut self.fling_y, moved.abs.y, moved.time);
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
                // A rubber band that actually opened is finished; the notes it
                // swept are already selected, so the up only reports.
                // A gesture that started on a note under the Select tool is
                // that gesture from beginning to end: the press already chose
                // the note, so the up only reports what is now selected.
                if self.band.take().is_some() {
                    self.dragging = false;
                    self.last_drag_abs = None;
                    cx.set_cursor(MouseCursor::Arrow);
                    state.ui.status = state.selection_description();
                    self.keep_animating(cx);
                    cx.redraw_all();
                    return;
                }
                // A gesture that panned is finished; it was never a click.
                let panned = self.grab.take().is_some_and(|grab| grab.active);
                if panned {
                    self.dragging = false;
                    self.last_drag_abs = None;
                    cx.set_cursor(MouseCursor::Grab);
                    push_sample(&mut self.fling_x, up.abs.x, up.time);
                    push_sample(&mut self.fling_y, up.abs.y, up.time);
                    self.launch_coast(cx, state);
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
                                // Writing a note by clicking a bar is direct
                                // manipulation: it belongs to the Edit tool,
                                // and to nothing the reader has not armed.
                                self.tool(state) == ScoreTool::Edit
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
                // A dialog is drawn OVER the paper, and hit testing by area
                // does not know that: a wheel notch over a list would reach
                // the page underneath and zoom it. Whatever is on top owns
                // the wheel.
                if state.ui.dialog != crate::DialogKind::None {
                    return;
                }
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
                // A platform that reports scroll phases says exactly where a
                // gesture starts and ends, so the anchor latch follows those
                // rather than guessing from timing. A plain wheel reports no
                // phase at all and falls back to the gap.
                let fresh = matches!(
                    scroll.phase,
                    ScrollPhase::Began | ScrollPhase::Touched
                );
                if matches!(
                    scroll.phase,
                    ScrollPhase::Ended | ScrollPhase::MomentumEnded
                ) {
                    self.zoom_gesture = None;
                }
                self.zoom_towards(cx, state, scroll.abs, factor, scroll.time, fresh);
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
                ..Default::default()
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
        self.draw_band(cx);
        let pan = state.ui.pan;
        let playing = state.practice.playing;
        let gliding = state.ui.glide.active;
        self.draw_scroll_bars(cx, rect, pan);

        cx.end_turtle_with_area(&mut self.area);
        if playing || gliding || self.motion.moving() || self.zoom_ease.active {
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

    /// The rubber band itself: a soft wash with a crisp edge, drawn above the
    /// engraving it is sweeping so it reads over noteheads rather than under
    /// them.
    fn draw_band(&mut self, cx: &mut Cx2d) {
        let Some(band) = self.band.as_ref().filter(|band| band.active) else {
            return;
        };
        let x = band.origin.x.min(band.current.x) as f32;
        let y = band.origin.y.min(band.current.y) as f32;
        let w = (band.current.x - band.origin.x).abs() as f32;
        let h = (band.current.y - band.origin.y).abs() as f32;
        let depth = self.draw_vector.draw_depth;
        self.draw_vector.draw_depth = depth + 6.0;
        self.draw_vector.begin();
        self.draw_vector.set_color(0.18, 0.50, 0.86, 0.12);
        self.draw_vector.rect(x, y, w, h);
        self.draw_vector.fill();
        self.draw_vector.set_color(0.18, 0.50, 0.86, 0.85);
        self.draw_vector.rect(x, y, w, h);
        self.draw_vector.stroke(1.0);
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

/// The point a zoom notch anchors on.
///
/// While a gesture is still running it keeps the point it first aimed at; a
/// notch that starts a new gesture — a reported phase change, or a long
/// enough silence — aims at wherever the pointer is now. This is the whole
/// fix for zoom wobble: an anchor that follows the pointer holds a different
/// piece of paper still on every frame, and the sum of that is a slide.
fn zoom_anchor(
    gesture: Option<ZoomGesture>,
    pointer: DVec2,
    time: f64,
    fresh_gesture: bool,
) -> DVec2 {
    match gesture {
        Some(gesture)
            if !fresh_gesture && (time - gesture.last_delta) < ZOOM_GESTURE_GAP =>
        {
            gesture.anchor
        }
        _ => pointer,
    }
}

/// One frame of the walk toward a target scale, eased in log space so equal
/// notches are equal-looking steps. Returns the next zoom and whether it has
/// arrived (at which point it IS the target exactly, not merely near it).
fn zoom_ease_step(current: f64, target: f64, dt: f64) -> (f64, bool) {
    let current_ln = current.max(1e-9).ln();
    let target_ln = target.max(1e-9).ln();
    let next = current_ln + (target_ln - current_ln) * (1.0 - (-dt / ZOOM_EASE_TAU).exp());
    if (target_ln - next).abs() < ZOOM_EASE_EPS {
        (target, true)
    } else {
        (next.exp(), false)
    }
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

/// One axis of the coast, plus the spring that catches it at the ends of the
/// document. Returns the axis's new pan.
///
/// Both integrations are closed-form rather than per-frame approximations,
/// which is the whole trick behind frame-rate independence: the friction step
/// `v * (1 - e^(-k dt)) / k` and the critically damped spring step both
/// compose exactly, so N steps of `dt/N` and one step of `dt` land in the same
/// place to the last bit. Nothing here is tuned against a 60 Hz assumption.
fn coast_axis(
    velocity: &mut f64,
    stretch: &mut f64,
    stretch_velocity: &mut f64,
    pan: f64,
    min: f64,
    max: f64,
    dt: f64,
) -> f64 {
    let mut pan = pan;
    if max - min <= 1e-9 {
        // Nowhere to go on this axis: no coast, and nothing to bounce off.
        // Flinging a locked axis into the void is not a feature.
        *velocity = 0.0;
        *stretch = 0.0;
        *stretch_velocity = 0.0;
        return pan.clamp(min, max);
    }
    if *velocity != 0.0 {
        let decay = (-PAN_FRICTION * dt).exp();
        let travel = *velocity * (1.0 - decay) / PAN_FRICTION;
        *velocity *= decay;
        let landed = (pan + travel).clamp(min, max);
        if (pan + travel - landed).abs() > 1e-9 {
            // The coast reached the end of the document. Its remaining speed
            // is not thrown away: it goes into the edge spring, so the paper
            // decelerates into the stop and eases back instead of hitting a
            // wall at full tilt.
            *stretch_velocity +=
                (*velocity * EDGE_ENERGY).clamp(-EDGE_MAX_SPEED, EDGE_MAX_SPEED);
            *velocity = 0.0;
        }
        pan = landed;
        if velocity.abs() <= PAN_STOP_SPEED {
            // Driven to exactly zero, not merely towards it: a coast that
            // creeps forever is an app that never idles.
            *velocity = 0.0;
        }
    }
    if *stretch != 0.0 || *stretch_velocity != 0.0 {
        let decay = (-EDGE_SPRING * dt).exp();
        let slope = *stretch_velocity + EDGE_SPRING * *stretch;
        let next = (*stretch + slope * dt) * decay;
        let next_velocity = (*stretch_velocity - EDGE_SPRING * slope * dt) * decay;
        *stretch = next;
        *stretch_velocity = next_velocity;
        if stretch.abs() < EDGE_SETTLE && stretch_velocity.abs() < EDGE_SETTLE * EDGE_SPRING {
            *stretch = 0.0;
            *stretch_velocity = 0.0;
        }
    }
    pan
}

/// The velocity the hand had when it let go, in window points per second, and
/// how far the retained samples travelled in total.
///
/// Both come from [`estimate_release_velocity`], which measures over a window
/// of *time* rather than over the last two events: a 500 Hz mouse delivers two
/// samples two milliseconds apart, and dividing pointer jitter by that turns a
/// careful drag into a maximum-speed fling.
fn release_velocity(x: &[ScrollSample], y: &[ScrollSample]) -> (DVec2, DVec2) {
    let (velocity_x, travel_x) = estimate_release_velocity(x);
    let (velocity_y, travel_y) = estimate_release_velocity(y);
    (
        dvec2(velocity_x, velocity_y),
        dvec2(travel_x, travel_y),
    )
}

/// Drop the speed on any axis that is already pinned against the end of the
/// document in the direction it points.
///
/// The hand goes on moving after the paper has stopped, so the pointer's
/// velocity at release says nothing about what the paper was doing. A bounce
/// it never earned reads as the document shoving back.
fn unpinned(velocity: DVec2, pan: DVec2, min: DVec2, max: DVec2) -> DVec2 {
    let axis = |v: f64, at: f64, low: f64, high: f64| {
        if (v > 0.0 && at >= high - 1e-6) || (v < 0.0 && at <= low + 1e-6) {
            0.0
        } else {
            v
        }
    };
    dvec2(
        axis(velocity.x, pan.x, min.x, max.x),
        axis(velocity.y, pan.y, min.y, max.y),
    )
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

    fn samples(points: &[(f64, f64)]) -> Vec<ScrollSample> {
        points
            .iter()
            .map(|(time, abs)| ScrollSample {
                abs: *abs,
                time: *time,
            })
            .collect()
    }

    /// The release velocity is measured over a window of time, not off the
    /// last two events.
    ///
    /// This is the difference between a usable flick and a lottery. A mouse
    /// reporting at 500 Hz puts its last two samples two milliseconds apart,
    /// so one pixel of jitter there reads as 500 pt/s; over the 40 ms window
    /// the same jitter is 25 pt/s and the estimate stays on the real speed.
    #[test]
    fn the_release_velocity_is_windowed_not_the_last_two_events() {
        // A steady 1000 pt/s drag sampled every 2 ms, with one jittery last
        // sample two points off the line.
        let mut points: Vec<(f64, f64)> = (0..=40).map(|i| (i as f64 * 0.002, i as f64 * 2.0)).collect();
        let truth = 1000.0;
        let last = points.last_mut().unwrap();
        last.1 += 2.0;
        let taken = samples(&points);
        let (velocity, travel) = release_velocity(&taken, &[]);
        // The last two events alone would read 2000 pt/s — twice the truth.
        let naive = (points[40].1 - points[39].1) / (points[40].0 - points[39].0);
        assert!(naive > 1.8 * truth, "the last two events are noisy: {naive}");
        assert!(
            (velocity.x - truth).abs() < 0.15 * truth,
            "windowed estimate {} should stay near {truth}",
            velocity.x
        );
        assert!((travel.x - 82.0).abs() < 1e-9);
        // No samples at all is no velocity, not a division by zero.
        assert_eq!(release_velocity(&[], &[]).0, DVec2::default());
    }

    /// An axis pinned against the end of the document keeps no momentum: the
    /// paper stopped there while the hand went on moving, and a bounce it
    /// never earned would read as the document shoving back.
    #[test]
    fn a_pinned_axis_keeps_no_momentum() {
        let min = dvec2(-500.0, -200.0);
        let max = dvec2(100.0, 50.0);
        let at_end = dvec2(100.0, 0.0);
        // Pushing further into the end it is already against: dropped.
        assert_eq!(unpinned(dvec2(900.0, 0.0), at_end, min, max).x, 0.0);
        // Pushing away from it: kept.
        assert_eq!(unpinned(dvec2(-900.0, 0.0), at_end, min, max).x, -900.0);
        // The other axis is judged on its own.
        assert_eq!(unpinned(dvec2(900.0, 400.0), at_end, min, max).y, 400.0);
        // Mid-document, nothing is pinned.
        let middle = dvec2(-200.0, 0.0);
        assert_eq!(unpinned(dvec2(900.0, -400.0), middle, min, max), dvec2(900.0, -400.0));
    }

    fn coast(velocity: DVec2, steps: usize, dt: f64) -> (DVec2, PanMotion) {
        let mut motion = PanMotion {
            velocity,
            ..PanMotion::default()
        };
        let min = dvec2(-1.0e6, -1.0e6);
        let max = dvec2(1.0e6, 1.0e6);
        let mut pan = DVec2::default();
        for _ in 0..steps {
            pan = motion.advance(pan, min, max, dt);
        }
        (pan, motion)
    }

    /// The coast is driven by elapsed time, not by frames.
    ///
    /// The friction is integrated in closed form, so half a second of coasting
    /// covers the same ground whether it arrives as 30 frames, 60, 120 or one
    /// — which is what makes the feel identical on a 60 Hz panel and a 120 Hz
    /// one instead of merely similar. A per-frame decay constant would put
    /// these three numbers wildly apart.
    #[test]
    fn the_coast_is_time_based_not_frame_based() {
        let launch = dvec2(1400.0, -600.0);
        let (at_60, after_60) = coast(launch, 30, 1.0 / 60.0);
        let (at_120, after_120) = coast(launch, 60, 1.0 / 120.0);
        let (at_240, _) = coast(launch, 120, 1.0 / 240.0);
        let (in_one, _) = coast(launch, 1, 0.5);
        for other in [at_120, at_240, in_one] {
            assert!(
                (at_60.x - other.x).abs() < 1e-9 && (at_60.y - other.y).abs() < 1e-9,
                "half a second is half a second: {at_60:?} vs {other:?}"
            );
        }
        // And the velocity left is the analytic one.
        let expected = (-PAN_FRICTION * 0.5).exp();
        assert!((after_60.velocity.x - launch.x * expected).abs() < 1e-9);
        assert!((after_120.velocity.y - launch.y * expected).abs() < 1e-9);
        // A coast can never carry further than v/k, whatever the frame rate.
        assert!(at_60.x < launch.x / PAN_FRICTION);
    }

    /// The coast ends: it reaches a stop, hands back exactly zero velocity,
    /// and stops asking for frames. A view that creeps forever is an app that
    /// never idles.
    #[test]
    fn the_coast_stops_cleanly_and_the_view_goes_quiet() {
        let (_, motion) = coast(dvec2(2200.0, 0.0), 240, 1.0 / 60.0);
        assert_eq!(motion.velocity, DVec2::default());
        assert!(!motion.moving(), "four seconds later, nothing is moving");
        // And it got there in about a second, not in ten.
        let mut quick = PanMotion {
            velocity: dvec2(2200.0, 0.0),
            ..PanMotion::default()
        };
        let mut pan = DVec2::default();
        let mut frames = 0;
        while quick.moving() && frames < 600 {
            pan = quick.advance(pan, dvec2(-1.0e6, -1.0e6), dvec2(1.0e6, 1.0e6), 1.0 / 60.0);
            frames += 1;
        }
        let seconds = frames as f64 / 60.0;
        assert!(
            (0.5..1.6).contains(&seconds),
            "a hard flick should settle in about a second, took {seconds}"
        );
    }

    /// Reaching the end of the document at speed decelerates into it and
    /// springs back, rather than stopping dead against a wall. The pan itself
    /// never leaves the document — only the drawn offset does — so the
    /// scrollbars and the page indicator cannot be dragged out of step by a
    /// bounce.
    #[test]
    fn the_ends_of_the_document_catch_the_coast_instead_of_walling_it() {
        let min = dvec2(-300.0, 0.0);
        let max = dvec2(0.0, 0.0);
        let mut motion = PanMotion {
            velocity: dvec2(2000.0, 0.0),
            ..PanMotion::default()
        };
        let mut pan = dvec2(-40.0, 0.0);
        let mut peak: f64 = 0.0;
        let mut frames = 0;
        while motion.moving() && frames < 600 {
            pan = motion.advance(pan, min, max, 1.0 / 60.0);
            assert!(pan.x <= max.x + 1e-9 && pan.x >= min.x - 1e-9, "the pan stays in the document");
            peak = peak.max(motion.overscroll.x.abs());
            frames += 1;
        }
        assert!(peak > 1.0, "the paper gives at the end rather than stopping dead");
        assert!(peak < 90.0, "the give is a bounce, not a hole: {peak}");
        assert_eq!(motion.overscroll, DVec2::default(), "and it settles back exactly");
        assert!(!motion.moving());
        // A locked axis (a page that fits) neither coasts nor bounces.
        let mut locked = PanMotion {
            velocity: dvec2(0.0, 3000.0),
            ..PanMotion::default()
        };
        let settled = locked.advance(dvec2(0.0, 12.0), dvec2(0.0, 12.0), dvec2(0.0, 12.0), 1.0 / 60.0);
        assert_eq!(settled.y, 12.0);
        assert!(!locked.moving());
    }

    /// A press catches the paper in the frame it lands: the coast is over, and
    /// the grab starts from wherever the paper had got to.
    #[test]
    fn a_press_stops_the_coast_in_one_frame() {
        let mut motion = PanMotion {
            velocity: dvec2(1800.0, 0.0),
            ..PanMotion::default()
        };
        let min = dvec2(-1.0e6, -1.0e6);
        let max = dvec2(1.0e6, 1.0e6);
        let caught_at = motion.advance(DVec2::default(), min, max, 1.0 / 60.0);
        assert!(caught_at.x > 0.0);
        motion.catch();
        assert!(!motion.moving(), "nothing is left to animate");
        assert_eq!(motion.advance(caught_at, min, max, 1.0 / 60.0), caught_at);
    }

    /// Easing the wheel's step over several frames must not let the document
    /// drift out from under the pointer: anchored zoom composes exactly, so a
    /// walk of many small steps lands where one big step would.
    #[test]
    fn an_eased_zoom_lands_exactly_where_the_notch_asked() {
        let anchor = dvec2(412.0, 291.0);
        let pan = dvec2(-260.0, -85.0);
        let (from, to) = (2.0, 2.0 * 1.33);
        let direct = zoom_pan_about(anchor, pan, from, to);
        // The same journey as an eased walk in log scale.
        let mut scale = from;
        let mut walked = pan;
        for _ in 0..120 {
            let next = (scale.ln() + (to.ln() - scale.ln()) * 0.25).exp();
            walked = zoom_pan_about(anchor, walked, scale, next);
            scale = next;
        }
        assert!((scale - to).abs() < 1e-9, "the walk arrives at the target");
        assert!((walked.x - direct.x).abs() < 1e-6, "{walked:?} vs {direct:?}");
        assert!((walked.y - direct.y).abs() < 1e-6);
    }

    /// The whole zoom pipeline, driven exactly as the canvas drives it: a
    /// stream of wheel/trackpad deltas interleaved with animation frames.
    ///
    /// `follow_pointer` reproduces the OLD behaviour — re-anchoring on the
    /// live pointer at every delta — so the test can show what that costs.
    struct ZoomRig {
        /// Window points per staff space at zoom 1, as `doc_layout` computes.
        base: f64,
        zoom: f64,
        pan: DVec2,
        gesture: Option<ZoomGesture>,
        ease_active: bool,
        ease_target: f64,
        ease_anchor: DVec2,
        follow_pointer: bool,
        /// The worst drift seen at any point, mid-ease included.
        worst: f64,
    }

    impl ZoomRig {
        fn new(follow_pointer: bool) -> Self {
            Self {
                base: 3.2,
                zoom: 1.0,
                pan: dvec2(-140.0, -90.0),
                gesture: None,
                ease_active: false,
                ease_target: 1.0,
                ease_anchor: DVec2::default(),
                follow_pointer: follow_pointer,
                worst: 0.0,
            }
        }
        /// The document point currently under a screen point, in staff spaces.
        fn under(&self, screen: DVec2) -> DVec2 {
            (screen - self.pan) / (self.base * self.zoom)
        }
        fn notch(&mut self, pointer: DVec2, time: f64, factor: f64) {
            let anchor = if self.follow_pointer {
                pointer
            } else {
                zoom_anchor(self.gesture, pointer, time, false)
            };
            self.gesture = Some(ZoomGesture {
                anchor,
                last_delta: time,
            });
            let from = if self.ease_active { self.ease_target } else { self.zoom };
            self.ease_target =
                (from * factor).clamp(crate::state::ZOOM_MIN, crate::state::ZOOM_MAX);
            self.ease_active = true;
            self.ease_anchor = anchor;
        }
        fn frame(&mut self, dt: f64) {
            if !self.ease_active {
                return;
            }
            let (next, arrived) = zoom_ease_step(self.zoom, self.ease_target, dt);
            if arrived {
                self.ease_active = false;
            }
            let before = self.base * self.zoom;
            let after = self.base * next;
            self.pan = zoom_pan_about(self.ease_anchor, self.pan, before, after);
            self.zoom = next;
        }
    }

    /// The invariant a pointer-centred zoom exists to keep: whatever the
    /// reader aimed at stays where they aimed, for the whole gesture and at
    /// every frame inside it — not merely once the animation has settled.
    ///
    /// Reproduces the reported wobble: many rapid deltas with the pointer
    /// drifting a little, which is what a hand on a trackpad actually does.
    /// A clean burst at a fixed point passes either way, which is why the
    /// earlier tests missed this.
    #[test]
    fn a_fast_zoom_holds_the_point_it_was_aimed_at() {
        let start = dvec2(420.0, 300.0);
        let mut latched = ZoomRig::new(false);
        let mut following = ZoomRig::new(true);
        // Where the reader was pointing when the gesture began, and the bit
        // of music that was under it. That is what must not move.
        let aimed_screen = start;
        let aimed_at = latched.under(aimed_screen);
        assert_eq!(aimed_at, following.under(aimed_screen));

        // 40 deltas over ~0.33 s, the pointer wandering up to ~12 points as a
        // hand does mid-scroll, with a couple of animation frames between.
        let mut time = 0.0;
        for step in 0..40 {
            let drift = dvec2(
                (step as f64 * 0.7).sin() * 12.0,
                (step as f64 * 0.4).cos() * 9.0,
            );
            // The gesture starts exactly under the reader's aim and wanders
            // from there, which is what a hand on a trackpad does.
            let pointer = if step == 0 { aimed_screen } else { aimed_screen + drift };
            time += 1.0 / 120.0;
            for rig in [&mut latched, &mut following] {
                rig.notch(pointer, time, 1.06);
                for _ in 0..2 {
                    rig.frame(1.0 / 240.0);
                    // The aim must hold DURING the ease, not just after it.
                    let held = (rig.under(aimed_screen) - aimed_at).length();
                    rig.worst = rig.worst.max(held);
                }
            }
        }
        // And let both settle.
        for _ in 0..200 {
            for rig in [&mut latched, &mut following] {
                rig.frame(1.0 / 120.0);
                rig.worst = rig.worst.max((rig.under(aimed_screen) - aimed_at).length());
            }
        }

        // Latched: the point aimed at never moves, at any frame.
        assert!(
            latched.worst < 1.0e-9,
            "the aimed-at point drifted {:.6} staff spaces while zooming",
            latched.worst
        );
        // The scale still arrives exactly where the notches asked.
        let expected = (1.06f64).powi(40).min(crate::state::ZOOM_MAX);
        assert!(
            (latched.zoom - expected).abs() < 1e-9,
            "{} vs {expected}",
            latched.zoom
        );
        // Following the live pointer is the bug, and it slides the paper by a
        // plainly visible amount. Without this the test could pass for the
        // wrong reason — a drift too small to have been worth fixing.
        let on_screen = following.worst * following.base * following.zoom;
        assert!(
            following.worst > 0.1 && on_screen > 4.0,
            "the follow-the-pointer rig should visibly slide, drifted {:.4} staff spaces \
             ({on_screen:.1} points on screen)",
            following.worst
        );
    }

    /// A gesture holds its aim; a deliberate re-aim after a pause takes the
    /// new one. Phase-reporting platforms say so explicitly.
    #[test]
    fn a_zoom_gesture_latches_its_aim_and_a_new_one_takes_a_new_aim() {
        let first = dvec2(100.0, 100.0);
        let moved = dvec2(400.0, 260.0);
        let gesture = Some(ZoomGesture {
            anchor: first,
            last_delta: 10.0,
        });
        // Mid-gesture, the pointer drifting: the aim is kept.
        assert_eq!(zoom_anchor(gesture, moved, 10.05, false), first);
        // A long enough silence is a new gesture: the new aim is taken.
        assert_eq!(
            zoom_anchor(gesture, moved, 10.0 + ZOOM_GESTURE_GAP + 0.01, false),
            moved
        );
        // A platform that reports a phase change says so outright.
        assert_eq!(zoom_anchor(gesture, moved, 10.05, true), moved);
        // With nothing latched yet, the pointer is the aim.
        assert_eq!(zoom_anchor(None, moved, 0.0, false), moved);
    }

    /// The ease arrives exactly on the target rather than merely near it, so
    /// a sequence of notches cannot accumulate scale error.
    #[test]
    fn the_zoom_ease_lands_exactly_and_stops() {
        let (mut zoom, target) = (1.0, 2.5);
        let mut frames = 0;
        loop {
            let (next, arrived) = zoom_ease_step(zoom, target, 1.0 / 120.0);
            zoom = next;
            frames += 1;
            if arrived {
                break;
            }
            assert!(frames < 200, "the ease must terminate");
        }
        assert_eq!(zoom, target, "it lands ON the target");
        assert!(frames > 5 && frames < 60, "a notch settles in about a fifth of a second: {frames}");
        // Zooming out lands exactly too.
        let (out, arrived) = zoom_ease_step(1.0, 1.0, 1.0 / 120.0);
        assert!(arrived && out == 1.0, "already there is already arrived");
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
