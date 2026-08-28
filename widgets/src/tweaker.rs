//! The TWEAKER — the design-feedback overlay every `--remote` app grows.
//!
//! Hardcoded into `Window` (like the caption bar: zero app wiring), inert
//! unless the remote bridge is live, zero cost while off. Turned on (F12 or
//! `GET /tweak?on=1`), a person points at the UI and live-edits it while the
//! AI watches the same session through the bridge:
//!
//! * pointer events over the window body are swallowed BEFORE ordinary
//!   widget dispatch (a Button under the cursor outlines, it never fires)
//!   and resolved against the live widget tree into a pick: id path, type,
//!   rect, margin/padding band;
//! * the selected widget's `#[source] ScriptObjectRef` is reflected (own
//!   map = explicitly set values, proto chain = the type's live/shader-input
//!   registry) into a real property list — no synthetic schema;
//! * splash chunks are live-loaded onto the one selected instance through
//!   the ordinary apply machinery (`script_apply_eval` semantics, `+:`
//!   merge rules intact) followed by a full redraw/relayout pass, and every
//!   applied chunk lands in the session diff log as (path, prop, old, new);
//! * `/tweak/final` answers the coalesced end state so the AI integrates
//!   once instead of tracking every intermediate edit.
//!
//! Containment (the plan of record, tweaker.md): everything UI-side lives
//! HERE; `Window` hosts the widget and calls [`window_intercept`] — a few
//! lines; the `/tweak` routes in `platform/src/remote.rs` stay thin and
//! delegate through `Cx::tweak_callback`, registered by `set_ui_root`
//! exactly like the widget-tree callbacks, so platform never depends on
//! widgets. The overlay reads and applies — only the AI writes source.

use crate::{
    check_box::{CheckBox, CheckBoxAction},
    fab_controls::{format_hex, parse_hex, FabColorPick, FabColorPickAction, FabValueInput, FabValueInputAction},
    label::Label,
    makepad_derive_widget::*,
    makepad_draw::*,
    portal_list::PortalList,
    text_input::TextInputAction,
    view::View,
    widget::*,
    widget_tree::{live_id_token, widget_type_names, CxWidgetExt},
};
use crate::makepad_platform::remote;
use crate::makepad_script::script_eval;
use crate::Animate;
use crate::ButtonAction;
use crate::makepad_script::trap::NoTrap;
use crate::makepad_script::{parse_doc_hint, ScriptHeap, ScriptMod, ScriptObject};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// session state — one tweak session per app, shared by every window's
// Tweaker instance and by the remote callback (all main-thread; the Mutex is
// for the static, not for concurrency).
// ---------------------------------------------------------------------------

/// Fast zero-cost-when-off gate: one relaxed atomic load per event.
static TWEAK_ON: AtomicBool = AtomicBool::new(false);

pub fn tweak_is_on() -> bool {
    TWEAK_ON.load(Ordering::Relaxed)
}

/// One resolved widget under the pointer (or pinned by a click).
#[derive(Clone, Debug, PartialEq)]
pub struct TweakPick {
    pub uid: u64,
    /// Dotted id path from the ui root, the same tokens `/d` prints.
    pub path: String,
    pub ty: String,
    /// Window-local rect.
    pub rect: Rect,
    pub window_id: usize,
    /// `Some("padding")` / `Some("margin:<child>")` when the pointer sits in
    /// the spacing band rather than on content — the gap between rects is a
    /// first-class target.
    pub band: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum PickStyle {
    Hover,
    Pinned,
    /// Pinned while a value is actively moving: hairline stipple only.
    PinnedQuiet,
}

#[derive(Clone, Debug)]
pub struct TweakDiffEntry {
    pub seq: u64,
    pub path: String,
    pub prop: String,
    pub old: String,
    pub new: String,
}

/// One freehand annotation stroke, in window-local points, tagged with the
/// widget paths it touches.
#[derive(Clone, Debug, Default)]
pub struct TweakStroke {
    pub window_id: usize,
    pub points: Vec<(f64, f64)>,
    pub widgets: Vec<String>,
}

#[derive(Default)]
struct TweakSession {
    /// Guards against N windows toggling N times on one F12 event.
    toggle_event_id: u64,
    /// The pinned selection (click pins; remote applies re-pin by path).
    pinned: Option<TweakPick>,
    hover: Option<TweakPick>,
    diff: Vec<TweakDiffEntry>,
    next_seq: u64,
    strokes: Vec<TweakStroke>,
    /// A stroke being drawn right now (annotate mode, mouse held down).
    live_stroke: Option<TweakStroke>,
    /// Annotate mode: drag draws instead of picking.
    annotate: bool,
    /// True once the user drew anything — `/tweak/final` then includes a
    /// composited screenshot so the AI sees what the drawings mean.
    drew: bool,
    /// A mouse-down we swallowed; swallow the matching up too.
    down_consumed: bool,
    /// Sidebar width in points (0 = use the default).
    sidebar_width: f64,
    /// The on-canvas selection outline hides until this time: an edit was
    /// applied within the last beat, so the widget must be seen exactly as
    /// it renders. Extended by every apply (sidebar or remote).
    suppress_until: f64,
    /// A sidebar interaction owns the canvas right now (field focused,
    /// color popover open): the outline stays hidden for its duration.
    edit_hold: bool,
    /// Throttle for sidebar TWEAK log lines: (path, prop, time) of the last
    /// emitted line, so a typing/scrubbing burst logs once per pause.
    last_sidebar_log: Option<(String, String, f64)>,
    /// Bumped by every applied change (any origin) and by resets/clears:
    /// the sidebar rebuilds its rows when it sees a new generation.
    apply_gen: u64,
}

fn session() -> &'static Mutex<TweakSession> {
    static S: OnceLock<Mutex<TweakSession>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(TweakSession::default()))
}

const DEFAULT_SIDEBAR_WIDTH: f64 = 280.0;
const SPLITTER_WIDTH: f64 = 5.0;

/// How long the selection outline stays quiet after the last applied edit.
const SUPPRESS_LINGER: f64 = 0.5;

fn sidebar_width() -> f64 {
    let width = session().lock().unwrap().sidebar_width;
    if width <= 0.0 {
        DEFAULT_SIDEBAR_WIDTH
    } else {
        width
    }
}

/// Turn the overlay on/off and force the full-app redraw that makes the
/// change visible everywhere.
pub fn set_tweak_on(cx: &mut Cx, on: bool) {
    let was = TWEAK_ON.swap(on, Ordering::Relaxed);
    if was != on {
        if !on {
            let mut s = session().lock().unwrap();
            s.hover = None;
            s.down_consumed = false;
            s.live_stroke = None;
        }
        log!("TWEAK mode {}", if on { "on" } else { "off" });
        cx.redraw_all();
    }
}

// ---------------------------------------------------------------------------
// pick resolution — the reflection walk's spine is the ordinary widget
// hierarchy: deepest visible widget whose clipped rect contains the point,
// later siblings (drawn on top) winning. `View.design_mode` is the revived
// container seam: a design-mode container is transparent to picking — its
// children resolve, it never does.
// ---------------------------------------------------------------------------

fn pick_candidate(cx: &Cx, widget: &WidgetRef, abs: Vec2d) -> Option<Rect> {
    if !widget.visible() {
        return None;
    }
    let rect = widget.area().clipped_rect(cx);
    if rect.size.x <= 0.0 || rect.size.y <= 0.0 || !rect.contains(abs) {
        return None;
    }
    Some(rect)
}

fn is_design_transparent(widget: &WidgetRef) -> bool {
    widget
        .borrow::<View>()
        .map_or(false, |view| view.design_mode())
}

fn walk_pick(
    cx: &Cx,
    widget: &WidgetRef,
    abs: Vec2d,
    depth: usize,
    best: &mut Option<(WidgetRef, Rect, usize)>,
) {
    if !widget.visible() {
        return;
    }
    if !is_design_transparent(widget) {
        if let Some(rect) = pick_candidate(cx, widget, abs) {
            let take = match best {
                // Deeper wins; equal depth: later in draw order wins.
                Some((_, _, best_depth)) => depth >= *best_depth,
                None => true,
            };
            if take {
                *best = Some((widget.clone(), rect, depth));
            }
        }
    }
    widget.children(&mut |_id, child| {
        walk_pick(cx, &child, abs, depth + 1, best);
    });
}

/// The spacing bands: when the picked widget is a container and the point
/// missed all of its children, say whether the point sits in the container's
/// padding ring or in a child's margin ring.
fn resolve_band(cx: &mut Cx, widget: &WidgetRef, rect: Rect, abs: Vec2d) -> Option<String> {
    let mut child_hit = false;
    let mut margin_of = None;
    widget.children(&mut |id, child| {
        if !child.visible() {
            return;
        }
        let child_rect = child.area().clipped_rect(cx);
        if child_rect.size.x <= 0.0 || child_rect.size.y <= 0.0 {
            return;
        }
        if child_rect.contains(abs) {
            child_hit = true;
            return;
        }
        let margin = child.walk(cx).margin;
        let expanded = Rect {
            pos: dvec2(
                child_rect.pos.x - margin.left,
                child_rect.pos.y - margin.top,
            ),
            size: dvec2(
                child_rect.size.x + margin.left + margin.right,
                child_rect.size.y + margin.top + margin.bottom,
            ),
        };
        if margin_of.is_none() && expanded.contains(abs) {
            margin_of = Some(format!("margin:{}", live_id_token(id)));
        }
    });
    if child_hit {
        return None;
    }
    if let Some(margin) = margin_of {
        return Some(margin);
    }
    // The gap between two adjacent siblings (flow spacing) is a first-class
    // target too: name both neighbours.
    let mut above: Option<(f64, LiveId)> = None;
    let mut below: Option<(f64, LiveId)> = None;
    let mut left: Option<(f64, LiveId)> = None;
    let mut right: Option<(f64, LiveId)> = None;
    widget.children(&mut |id, child| {
        if !child.visible() {
            return;
        }
        let r = child.area().clipped_rect(cx);
        if r.size.x <= 0.0 || r.size.y <= 0.0 {
            return;
        }
        let x_overlaps = abs.x >= r.pos.x && abs.x <= r.pos.x + r.size.x;
        let y_overlaps = abs.y >= r.pos.y && abs.y <= r.pos.y + r.size.y;
        if x_overlaps {
            let bottom = r.pos.y + r.size.y;
            if bottom <= abs.y && above.as_ref().map_or(true, |(edge, _)| bottom > *edge) {
                above = Some((bottom, id));
            }
            if r.pos.y >= abs.y && below.as_ref().map_or(true, |(edge, _)| r.pos.y < *edge) {
                below = Some((r.pos.y, id));
            }
        }
        if y_overlaps {
            let edge = r.pos.x + r.size.x;
            if edge <= abs.x && left.as_ref().map_or(true, |(e, _)| edge > *e) {
                left = Some((edge, id));
            }
            if r.pos.x >= abs.x && right.as_ref().map_or(true, |(e, _)| r.pos.x < *e) {
                right = Some((r.pos.x, id));
            }
        }
    });
    if let (Some((_, a)), Some((_, b))) = (&above, &below) {
        return Some(format!("gap:{}~{}", live_id_token(*a), live_id_token(*b)));
    }
    if let (Some((_, a)), Some((_, b))) = (&left, &right) {
        return Some(format!("gap:{}~{}", live_id_token(*a), live_id_token(*b)));
    }
    if let Some(view) = widget.borrow::<View>() {
        let padding = view.layout.padding;
        let content = Rect {
            pos: dvec2(rect.pos.x + padding.left, rect.pos.y + padding.top),
            size: dvec2(
                rect.size.x - padding.left - padding.right,
                rect.size.y - padding.top - padding.bottom,
            ),
        };
        if (padding.left != 0.0
            || padding.right != 0.0
            || padding.top != 0.0
            || padding.bottom != 0.0)
            && !content.contains(abs)
        {
            return Some("padding".to_string());
        }
    }
    None
}

fn resolve_pick(
    cx: &mut Cx,
    root: &WidgetRef,
    abs: Vec2d,
    window_id: usize,
) -> Option<TweakPick> {
    let mut best = None;
    // Start below the root (the window body itself is chrome, not content).
    root.children(&mut |_id, child| {
        walk_pick(cx, &child, abs, 0, &mut best);
    });
    let (widget, rect, _depth) = best?;
    let uid = widget.widget_uid();
    let path_ids = cx.widget_tree().path_to(uid);
    let path = if path_ids.is_empty() {
        format!("uid:{}", uid.0)
    } else {
        path_ids
            .iter()
            .map(|id| live_id_token(*id))
            .collect::<Vec<_>>()
            .join(".")
    };
    let ty = widget
        .widget_type_id()
        .and_then(|type_id| widget_type_names(cx).get(&type_id).copied())
        .map(live_id_token)
        .unwrap_or_else(|| "-".to_string());
    let band = resolve_band(cx, &widget, rect, abs);
    Some(TweakPick {
        uid: uid.0,
        path,
        ty,
        rect,
        window_id,
        band,
    })
}

// ---------------------------------------------------------------------------
// the Window seam — swallow pointer events before ordinary dispatch while
// the overlay is on, so picking can never activate the app's widgets.
// ---------------------------------------------------------------------------

/// Called by `Window::handle_event` in place of ordinary dispatch. Returns
/// `true` when the event was swallowed (the window must NOT hand it to its
/// view children). Off: one atomic load (plus an F12 check on key events).
pub fn window_intercept(
    cx: &mut Cx,
    event: &Event,
    window_view: &mut View,
    window_id: WindowId,
) -> bool {
    // F12 toggles the mode — only while the remote bridge is live: the
    // tweaker is a --remote feature and stays fully inert without it.
    if let Event::KeyDown(key_event) = event {
        if key_event.key_code == KeyCode::F12 && remote::is_active() {
            let flip = {
                let mut s = session().lock().unwrap();
                if s.toggle_event_id != cx.event_id() {
                    s.toggle_event_id = cx.event_id();
                    true
                } else {
                    false
                }
            };
            if flip {
                set_tweak_on(cx, !tweak_is_on());
            }
            return true;
        }
        return false;
    }
    if !tweak_is_on() {
        return false;
    }

    let (abs, kind) = match event {
        Event::MouseMove(e) if e.window_id == window_id => (e.abs, PointerKind::Move),
        Event::MouseDown(e) if e.window_id == window_id => (e.abs, PointerKind::Down),
        Event::MouseUp(e) if e.window_id == window_id => (e.abs, PointerKind::Up),
        Event::Scroll(e) if e.window_id == window_id => (e.abs, PointerKind::Scroll),
        _ => return false,
    };

    let body = window_view
        .children
        .iter()
        .find(|(id, _)| *id == live_id!(body))
        .map(|(_, widget)| widget.clone());
    let tweaker = window_view
        .children
        .iter()
        .find(|(id, _)| *id == live_id!(tweaker))
        .map(|(_, widget)| widget.clone());
    let (Some(body), Some(tweaker)) = (body, tweaker) else {
        return false;
    };
    let body_rect = body.area().clipped_rect(cx);

    // A splitter drag owns the pointer outright: the pick path must not
    // eat the Move that resizes or the Up that releases — releasing
    // OUTSIDE the band swallowed the Up and wedged the drag forever.
    {
        let dragging = tweaker
            .borrow::<Tweaker>()
            .map(|tw| tw.splitter_drag)
            .unwrap_or(false);
        if dragging {
            return false;
        }
    }
    // A mouse-up always pairs with the down that started it: if we swallowed
    // the down, swallow the up wherever it lands.
    let finish_consumed_down = kind == PointerKind::Up && session().lock().unwrap().down_consumed;
    if !body_rect.contains(abs) && !finish_consumed_down {
        // Outside the body (caption bar, sidebar band): ordinary dispatch.
        return false;
    }
    // The tweaker's own UI is never a pick target. A body that was not
    // vacated (apps whose layout ignores the sidebar apply) still overlaps
    // the band, so check it explicitly: a pointer over the panel goes to
    // ordinary dispatch — the panel wins INPUT, and the app widget behind
    // it can never be selected through it.
    if !finish_consumed_down {
        let in_band = tweaker
            .borrow::<Tweaker>()
            .map(|tw| tw.band.size.x > 0.0 && tw.band.contains(abs))
            .unwrap_or(false);
        if in_band {
            return false;
        }
    }

    // Annotate mode draws; holding Alt draws too, so a person can sketch a
    // note mid-pick without flipping the mode.
    let alt_held = match event {
        Event::MouseMove(e) => e.modifiers.alt,
        Event::MouseDown(e) => e.modifiers.alt,
        Event::MouseUp(e) => e.modifiers.alt,
        _ => false,
    };
    let annotate = session().lock().unwrap().annotate || alt_held;

    // Direct manipulation first: the corner handles of a radius-carrying
    // selection own their presses before picking does.
    if !annotate {
        match kind {
            PointerKind::Down => {
                let pinned = session().lock().unwrap().pinned.clone();
                if let Some(pin) = pinned.filter(|p| p.window_id == window_id.id()) {
                    let hit = {
                        let tw = tweaker.borrow::<Tweaker>();
                        tw.and_then(|tw| {
                            tw.radius_prop.clone().and_then(|(prop, value)| {
                                Tweaker::radius_handle_centers(pin.rect)
                                    .iter()
                                    .position(|center| {
                                        let dx = abs.x - center.x;
                                        let dy = abs.y - center.y;
                                        dx * dx + dy * dy <= 49.0
                                    })
                                    .map(|corner| (corner, prop, value))
                            })
                        })
                    };
                    if let Some((corner, _prop, value)) = hit {
                        if let Some(mut tw) = tweaker.borrow_mut::<Tweaker>() {
                            tw.radius_drag = Some((corner, value, abs));
                        }
                        {
                            let mut s = session().lock().unwrap();
                            s.down_consumed = true;
                            s.edit_hold = true;
                        }
                        redraw_tweaker(cx, &tweaker);
                        return true;
                    }
                }
            }
            PointerKind::Move => {
                let drag = tweaker
                    .borrow::<Tweaker>()
                    .and_then(|tw| tw.radius_drag.map(|d| (d, tw.radius_prop.clone())));
                if let Some(((corner, start_value, start_pos), Some((prop, _)))) = drag {
                    let inward = Tweaker::radius_inward(corner);
                    let delta = dvec2(abs.x - start_pos.x, abs.y - start_pos.y);
                    let travel = (delta.x * inward.x + delta.y * inward.y) * 0.5;
                    let value = (start_value + travel).max(0.0);
                    let sel = session().lock().unwrap().pinned.clone();
                    if let Some(sel) = sel {
                        let widget = cx.widget_tree().widget(WidgetUid(sel.uid));
                        if !widget.is_empty() {
                            let chunk = format!("{}: {}", prop, fmt_f64(value));
                            if let Err(error) =
                                apply_splash_chunk(cx, &widget, &sel.path, &chunk, "handle")
                            {
                                log!("TWEAK handle apply failed: {error}");
                            }
                        }
                    }
                    cx.set_cursor(MouseCursor::Crosshair);
                    redraw_tweaker(cx, &tweaker);
                    return true;
                }
            }
            PointerKind::Up => {
                let was_dragging = tweaker
                    .borrow::<Tweaker>()
                    .is_some_and(|tw| tw.radius_drag.is_some());
                if was_dragging {
                    if let Some(mut tw) = tweaker.borrow_mut::<Tweaker>() {
                        tw.radius_drag = None;
                        tw.rows_uid = 0;
                    }
                    {
                        let mut s = session().lock().unwrap();
                        s.down_consumed = false;
                        s.edit_hold = false;
                    }
                    redraw_tweaker(cx, &tweaker);
                    return true;
                }
            }
            PointerKind::Scroll => {}
        }
    }

    match kind {
        PointerKind::Move => {
            cx.set_cursor(if annotate {
                MouseCursor::Crosshair
            } else {
                MouseCursor::Hand
            });
            let drawing = session().lock().unwrap().live_stroke.is_some();
            if drawing {
                let mut s = session().lock().unwrap();
                if let Some(stroke) = &mut s.live_stroke {
                    stroke.points.push((abs.x, abs.y));
                }
                drop(s);
                redraw_tweaker(cx, &tweaker);
            } else {
                let pick = resolve_pick(cx, &body, abs, window_id.id());
                let mut s = session().lock().unwrap();
                if s.hover != pick {
                    s.hover = pick;
                    drop(s);
                    redraw_tweaker(cx, &tweaker);
                }
            }
        }
        PointerKind::Down => {
            {
                session().lock().unwrap().down_consumed = true;
            }
            // A sidebar interaction (color popover, focused field) owns this
            // press: hand it to the tweaker so the popover closes / the
            // field commits — the selection does NOT change.
            if session().lock().unwrap().edit_hold {
                tweaker.handle_event(cx, event, &mut Scope::empty());
                // The press ends the interaction whatever the widgets made
                // of it (a popover that closed already said so; a focused
                // field may not report focus loss for an off-widget press).
                session().lock().unwrap().edit_hold = false;
                redraw_tweaker(cx, &tweaker);
                return true;
            }
            if annotate {
                let mut stroke = TweakStroke::default();
                stroke.window_id = window_id.id();
                stroke.points.push((abs.x, abs.y));
                if let Some(pick) = resolve_pick(cx, &body, abs, window_id.id()) {
                    stroke.widgets.push(pick.path);
                }
                session().lock().unwrap().live_stroke = Some(stroke);
            } else {
                let pick = resolve_pick(cx, &body, abs, window_id.id());
                let mut s = session().lock().unwrap();
                match &pick {
                    Some(pick) => {
                        log!(
                            "TWEAK pick {} ({}) rect {:.0},{:.0} {:.0}x{:.0}{}",
                            pick.path,
                            pick.ty,
                            pick.rect.pos.x,
                            pick.rect.pos.y,
                            pick.rect.size.x,
                            pick.rect.size.y,
                            match &pick.band {
                                Some(band) => format!(" band {band}"),
                                None => String::new(),
                            }
                        );
                        s.pinned = Some(pick.clone());
                    }
                    None => {
                        s.pinned = None;
                    }
                }
                drop(s);
                sidebar_refresh(cx, &tweaker);
                redraw_tweaker(cx, &tweaker);
            }
        }
        PointerKind::Up => {
            let mut s = session().lock().unwrap();
            s.down_consumed = false;
            if let Some(mut stroke) = s.live_stroke.take() {
                stroke.points.push((abs.x, abs.y));
                // Tag the widgets the stroke touches: resolve its endpoints
                // and midpoint.
                s.strokes.push(stroke);
                s.drew = true;
                let stroke_index = s.strokes.len() - 1;
                drop(s);
                tag_stroke_widgets(cx, &body, window_id.id(), stroke_index);
                redraw_tweaker(cx, &tweaker);
            }
        }
        PointerKind::Scroll => {
            // Swallowed: scrolling would move the thing being pointed at.
        }
    }
    true
}

#[derive(Clone, Copy, PartialEq)]
enum PointerKind {
    Move,
    Down,
    Up,
    Scroll,
}

fn redraw_tweaker(cx: &mut Cx, tweaker: &WidgetRef) {
    if let Some(mut tweaker) = tweaker.borrow_mut::<Tweaker>() {
        tweaker.redraw_overlay(cx);
    }
}

fn sidebar_refresh(cx: &mut Cx, tweaker: &WidgetRef) {
    if let Some(mut tweaker) = tweaker.borrow_mut::<Tweaker>() {
        // Force the rows to rebuild on the next draw, even for a re-pin of
        // the same widget (its values may have moved underneath).
        tweaker.rows_uid = 0;
        let _ = cx;
    }
}

fn tag_stroke_widgets(cx: &mut Cx, body: &WidgetRef, window_id: usize, stroke_index: usize) {
    let points: Vec<(f64, f64)> = {
        let s = session().lock().unwrap();
        match s.strokes.get(stroke_index) {
            Some(stroke) => stroke.points.clone(),
            None => return,
        }
    };
    let mut widgets: Vec<String> = Vec::new();
    // Sample the stroke sparsely; resolving every point would walk the tree
    // hundreds of times for one squiggle.
    let step = (points.len() / 8).max(1);
    for (index, (x, y)) in points.iter().enumerate() {
        if index % step != 0 && index != points.len() - 1 {
            continue;
        }
        if let Some(pick) = resolve_pick(cx, body, dvec2(*x, *y), window_id) {
            if !widgets.contains(&pick.path) {
                widgets.push(pick.path);
            }
        }
    }
    let mut s = session().lock().unwrap();
    if let Some(stroke) = s.strokes.get_mut(stroke_index) {
        stroke.widgets = widgets;
    }
}

// ---------------------------------------------------------------------------
// reflection — walk the selected widget's applied script object (own map =
// explicitly set, proto chain = the type's live/shader-input registry) into
// a property list with real current values.
// ---------------------------------------------------------------------------

/// Keys that are structure, not style: reflected objects skip them.
fn skip_key(name: &str) -> bool {
    matches!(name, "animator")
}

/// Draw-shader plumbing the GPU pipeline owns — never user style. Skipped in
/// the dotted expansion so the sidebar shows design inputs, not internals.
fn is_shader_plumbing(name: &str) -> bool {
    matches!(
        name,
        "rect_pos"
            | "rect_size"
            | "draw_clip"
            | "delta_clip"
            | "draw_depth"
            | "draw_zbias"
            | "char_index"
            | "texture_index"
            | "atlas_plane"
            | "t"
            | "t_min"
            | "t_max"
            | "temp_y_shift"
            | "aa_2x2"
            | "aa_4x4"
            | "pad1"
            | "pad2"
            | "font_scale"
            | "depth_clip"
            | "debug"
            | "stem_darken_max"
            | "draw_scroll"
            | "view_shift"
            | "view_clip"
            | "camera_projection"
            | "camera_view"
            | "camera_inv"
            | "dpi_factor"
            | "dpi_dilate"
            | "seed"
            | "pos"
            | "vertex_pos"
            | "total_chars"
            | "char_depth"
            | "draw_call"
            | "draw_list"
            | "draw_pass"
            | "geom"
            | "world"
            | "extend_area"
            | "ink_centered"
            | "sdf_sharpness"
            | "sdf_luma_bias"
            | "draw_flags"
            | "clip_x"
            | "clip_y"
    )
}

fn fmt_scalar(heap: &ScriptHeap, value: ScriptValue) -> Option<String> {
    if value.is_nil() {
        return Some("null".to_string());
    }
    if let Some(color) = value.as_color() {
        return Some(format!("#{color:08x}"));
    }
    if let Some(b) = value.as_bool() {
        return Some(if b { "true" } else { "false" }.to_string());
    }
    if let Some(n) = value.as_number() {
        if n.fract() == 0.0 && n.abs() < 1.0e15 {
            return Some(format!("{}", n as i64));
        }
        return Some(format!("{}", (n * 10000.0).round() / 10000.0));
    }
    if let Some(id) = value.as_id() {
        return Some(format!("@{}", live_id_token(id)));
    }
    if value.is_string_like() {
        if let Some(text) = heap.string_with(value, |_, s| format!("{s:?}")) {
            return Some(text);
        }
        return Some("\"\"".to_string());
    }
    if let Some(pod) = value.as_pod() {
        let (pod_type, data) = heap.pod_data(pod);
        let name = pod_type
            .name
            .map(live_id_token)
            .unwrap_or_else(|| "pod".to_string());
        // A unit-range vec4f in UI code is a color; render it the way the
        // splash source would spell it, so diffs paste straight back.
        if name == "vec4f" && data.len() == 4 {
            let mut components = [0.0f32; 4];
            let mut unit = true;
            for (index, raw) in data.iter().enumerate() {
                let v = f32::from_bits(*raw);
                if !(0.0..=1.0).contains(&v) {
                    unit = false;
                    break;
                }
                components[index] = v;
            }
            if unit {
                let to_byte = |v: f32| (v * 255.0).round() as u32;
                return Some(format!(
                    "#{:02x}{:02x}{:02x}{:02x}",
                    to_byte(components[0]),
                    to_byte(components[1]),
                    to_byte(components[2]),
                    to_byte(components[3])
                ));
            }
        }
        let mut out = format!("{name}(");
        for (index, raw) in data.iter().enumerate() {
            if index > 0 {
                out.push(' ');
            }
            let v = f32::from_bits(*raw);
            if v.fract() == 0.0 {
                out.push_str(&format!("{}", v as i64));
            } else {
                out.push_str(&format!("{v}"));
            }
        }
        out.push(')');
        return Some(out);
    }
    None
}

/// Splash-ish one-line rendering of a value, `depth` levels of objects deep.
fn fmt_value(heap: &ScriptHeap, value: ScriptValue, depth: usize) -> Option<String> {
    if let Some(text) = fmt_scalar(heap, value) {
        return Some(text);
    }
    if let Some(array) = value.as_array() {
        let mut out = String::from("[");
        let len = heap.array_len(array).min(8);
        for index in 0..len {
            if index > 0 {
                out.push(' ');
            }
            let item = heap.array_index(array, index, NoTrap);
            match fmt_value(heap, item, depth.saturating_sub(1)) {
                Some(text) => out.push_str(&text),
                None => out.push('?'),
            }
        }
        out.push(']');
        return Some(out);
    }
    if let Some(obj) = value.as_object() {
        if heap.as_fn(obj).is_some() {
            return None; // functions are not properties
        }
        // `instance(v)` / `uniform(v)` shader-io wrappers carry the value in
        // their proto slot — unwrap so shader inputs read as plain values.
        if heap.as_shader_io(obj).is_some() {
            let inner = heap.proto(obj);
            if inner.is_nil() {
                return None; // texture/buffer declarations, not values
            }
            return fmt_value(heap, inner, depth);
        }
        if depth == 0 {
            return Some("{..}".to_string());
        }
        let mut keys: Vec<LiveId> = Vec::new();
        collect_prop_keys(heap, obj, &mut keys);
        let mut out = String::from("{");
        let mut first = true;
        for key in keys.into_iter().take(24) {
            let value = heap.value(obj, key.into(), NoTrap);
            let Some(text) = fmt_value(heap, value, depth - 1) else {
                continue;
            };
            if !first {
                out.push(' ');
            }
            first = false;
            out.push_str(&live_id_token(key));
            out.push_str(": ");
            out.push_str(&text);
        }
        out.push('}');
        return Some(out);
    }
    None
}

/// The union of map keys over the object and its proto chain, leaf first —
/// exactly the widget's live surface: what was applied plus every default
/// the type registered.
fn collect_prop_keys(heap: &ScriptHeap, obj: ScriptObject, keys: &mut Vec<LiveId>) {
    let mut ptr = obj;
    let mut hops = 0;
    loop {
        heap.map_ref(ptr).iter().for_each(|(key, _map_value)| {
            if let Some(id) = key.as_id() {
                if !keys.contains(&id) {
                    let name = live_id_token(id);
                    if !name.starts_with("__") && !skip_key(&name) {
                        keys.push(id);
                    }
                }
            }
        });
        match heap.proto(ptr).as_object() {
            Some(next) => {
                ptr = next;
                hops += 1;
                if hops > 24 {
                    break;
                }
            }
            None => break,
        }
    }
}

/// A value rendering that carries no information for the sidebar or diff.
fn is_noise(text: &str) -> bool {
    matches!(text, "{}" | "{..}" | "null")
}

/// Flat `name -> value` reflection of a widget's CURRENT state, one level of
/// nesting expanded with dotted names (`draw_bg.color`). Values come from
/// `script_to_value` — the Rust fields serialized back to script — so runtime
/// applies are visible; the `#[source]` object's own map (what the DSL
/// explicitly applied) supplies the `set` flag. This is both the sidebar's
/// data and the before/after capture the diff log works from.
fn reflect_flat(cx: &mut Cx, widget: &WidgetRef) -> Vec<(String, String, bool)> {
    cx.with_vm(|vm| {
        // Serializing a widget back to script trips harmless type-check
        // complaints on fn-ref fields (`on_click` serializes to a value its
        // own type check rejects). Reflection is read-only — capture and
        // drop them instead of spamming the log on every state read.
        vm.bx.captured_errors = Some(Vec::new());
        let current = widget.current_to_value(vm);
        let _ = vm.take_errors();
        let Some(current_obj) = current.as_object() else {
            return Vec::new();
        };
        let source = widget.script_source();
        let heap = &vm.bx.heap;
        let own: Vec<LiveId> = if source == ScriptObject::ZERO {
            Vec::new()
        } else {
            heap.map_ref(source)
                .iter()
                .filter_map(|(key, _)| key.as_id())
                .collect()
        };
        let mut out = Vec::new();
        // Only the object's OWN map: script_to_value wrote every live field
        // explicitly, the proto behind it is just the type object.
        let keys: Vec<LiveId> = heap
            .map_ref(current_obj)
            .iter()
            .filter_map(|(key, _)| key.as_id())
            .filter(|id| {
                let name = live_id_token(*id);
                !name.starts_with("__") && !skip_key(&name)
            })
            .collect();
        for key in keys {
            let value = heap.value(current_obj, key.into(), NoTrap);
            let is_set = own.contains(&key);
            let name = live_id_token(key);
            if let Some(obj) = value.as_object() {
                if heap.as_fn(obj).is_some() {
                    continue;
                }
                // One level of dotted expansion for typed sub-structs
                // (draw_bg.color, padding.left ...), own map only.
                let sub_keys: Vec<LiveId> = heap
                    .map_ref(obj)
                    .iter()
                    .filter_map(|(sub_key, _)| sub_key.as_id())
                    .filter(|id| {
                        let name = live_id_token(*id);
                        !name.starts_with("__") && !is_shader_plumbing(&name)
                    })
                    .collect();
                let had_subs = !sub_keys.is_empty();
                let mut wrote_sub = false;
                for sub_key in sub_keys.into_iter().take(200) {
                    let sub_value = heap.value(obj, sub_key.into(), NoTrap);
                    if sub_value.as_object().is_some_and(|o| heap.as_fn(o).is_some()) {
                        continue;
                    }
                    if let Some(text) = fmt_value(heap, sub_value, 1) {
                        if is_noise(&text) {
                            continue;
                        }
                        out.push((format!("{name}.{}", live_id_token(sub_key)), text, is_set));
                        wrote_sub = true;
                    }
                }
                if !wrote_sub && !had_subs {
                    if let Some(text) = fmt_value(heap, value, 1) {
                        if !is_noise(&text) {
                            out.push((name, text, is_set));
                        }
                    }
                }
            } else if let Some(text) = fmt_value(heap, value, 1) {
                if !is_noise(&text) || is_set {
                    out.push((name, text, is_set));
                }
            }
            if out.len() > 400 {
                break;
            }
        }
        // Second pass: the type's shader-input registry. Instance/uniform
        // inputs declared in the DSL (`border_radius: instance(2.0)`) are
        // not Rust fields, so `script_to_value` never sees them — but they
        // live in the source object's proto chain. Union them in (source
        // values are the pre-tweak truth; tweaks to them land in the diff).
        if source != ScriptObject::ZERO {
            let mut keys = Vec::new();
            collect_prop_keys(heap, source, &mut keys);
            for key in keys {
                let name = live_id_token(key);
                if is_shader_plumbing(&name) {
                    continue;
                }
                let value = heap.value(source, key.into(), NoTrap);
                if let Some(obj) = value.as_object() {
                    if heap.as_fn(obj).is_some() {
                        continue;
                    }
                    let mut sub_keys = Vec::new();
                    collect_prop_keys(heap, obj, &mut sub_keys);
                    sub_keys.retain(|id| {
                        let name = live_id_token(*id);
                        !name.starts_with("__") && !is_shader_plumbing(&name)
                    });
                    for sub_key in sub_keys.into_iter().take(200) {
                        let sub_name = live_id_token(sub_key);
                        let dotted = format!("{name}.{sub_name}");
                        if out.iter().any(|(existing, _, _)| *existing == dotted) {
                            continue;
                        }
                        let sub_value = heap.value(obj, sub_key.into(), NoTrap);
                        if sub_value
                            .as_object()
                            .is_some_and(|o| heap.as_fn(o).is_some())
                        {
                            continue;
                        }
                        if let Some(text) = fmt_value(heap, sub_value, 1) {
                            if !is_noise(&text) {
                                out.push((dotted, text, false));
                            }
                        }
                    }
                } else {
                    if out.iter().any(|(existing, _, _)| *existing == name) {
                        continue;
                    }
                    if let Some(text) = fmt_value(heap, value, 1) {
                        if !is_noise(&text) {
                            out.push((name, text, false));
                        }
                    }
                }
                if out.len() > 400 {
                    break;
                }
            }
        }
        out
    })
}

// ---------------------------------------------------------------------------
// live-load — evaluate a splash chunk and apply it onto the one selected
// instance through the ordinary apply machinery, then a full redraw/relayout.
// ---------------------------------------------------------------------------

/// One level of the selection's construction chain, display-ready.
/// Built from `vm.construction_chain` over the widget's `#[source]` object:
/// the proto chain of `made_at` ips, each resolved to a source location and
/// its `///` docs (see platform/script/src/docs.rs).
struct CascadeLevel {
    /// "button.rs:52" (basename:line), or "native" for Rust-built levels.
    loc: String,
    /// Full file path for click-through / the AI ("" for native levels).
    file: String,
    line: u32,
    /// `///` doc attached to the level's object literal.
    doc: Option<String>,
    /// `///` docs attached to fields of that literal.
    field_docs: Vec<(String, String)>,
    /// Keys the level sets itself; true = overridden by a closer level.
    sets: Vec<(String, bool)>,
}

/// Doc-channel text per row prop: field docs from the widget's own
/// construction chain plus one dotted level into its object-valued keys
/// (draw layers, typed sub-structs) — `draw_bg.border_size` finds the
/// `/** bevel border thickness 0..4 step 0.5 */` written inside the
/// draw_bg literal. Closest level wins. Feeds tooltips and the
/// hints->scrubber wiring (`parse_doc_hint`).
fn collect_row_docs(cx: &mut Cx, widget: &WidgetRef) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let source = widget.script_source();
    if source == ScriptObject::ZERO {
        return out;
    }
    cx.with_vm(|vm| {
        let chain = vm.construction_chain(source.into());
        for lvl in &chain {
            for (f, d) in &lvl.field_docs {
                out.entry(live_id_token(*f)).or_insert_with(|| d.clone());
            }
        }
        for lvl in &chain {
            for key in &lvl.own_keys {
                let value = vm.bx.heap.value(lvl.object, (*key).into(), NoTrap);
                let Some(obj) = value.as_object() else { continue };
                if vm.bx.heap.as_fn(obj).is_some() {
                    continue;
                }
                let name = live_id_token(*key);
                for sub in vm.construction_chain(value) {
                    if let Some(doc) = &sub.doc {
                        out.entry(name.clone()).or_insert_with(|| doc.clone());
                    }
                    for (f, d) in &sub.field_docs {
                        out.entry(format!("{name}.{}", live_id_token(*f)))
                            .or_insert_with(|| d.clone());
                    }
                }
            }
        }
    });
    out
}

fn cascade_levels(cx: &mut Cx, widget: &WidgetRef) -> Vec<CascadeLevel> {
    let source = widget.script_source();
    if source == ScriptObject::ZERO {
        return Vec::new();
    }
    cx.with_vm(|vm| {
        let chain = vm.construction_chain(source.into());
        let mut seen: std::collections::HashSet<String> = Default::default();
        let mut out = Vec::new();
        for lvl in chain {
            let (loc, file, line) = match &lvl.loc {
                Some(l) => {
                    let base = l.file.rsplit('/').next().unwrap_or(&l.file);
                    (format!("{base}:{}", l.line), l.file.clone(), l.line)
                }
                None => ("native".to_string(), String::new(), 0),
            };
            let mut sets: Vec<(String, bool)> = Vec::new();
            for key in &lvl.own_keys {
                let name = live_id_token(*key);
                if name.starts_with("__") || skip_key(&name) {
                    continue;
                }
                let overridden = seen.contains(&name);
                sets.push((name, overridden));
            }
            for (name, _) in &sets {
                seen.insert(name.clone());
            }
            out.push(CascadeLevel {
                loc,
                file,
                line,
                doc: lvl.doc.clone(),
                field_docs: lvl
                    .field_docs
                    .iter()
                    .map(|(f, d)| (live_id_token(*f), d.clone()))
                    .collect(),
                sets,
            });
        }
        out
    })
}

fn resolve_widget_by_path(cx: &Cx, path: &str) -> Result<WidgetRef, String> {
    let tree = cx.widget_tree();
    let ids: Vec<LiveId> = path
        .split('.')
        .filter(|segment| !segment.is_empty() && *segment != "-")
        .map(LiveId::from_str)
        .collect();
    if ids.is_empty() {
        return Err("empty path".to_string());
    }
    let root = tree.root_uid();
    let found = tree.find_within(root, &ids);
    if !found.is_empty() {
        return Ok(found);
    }
    // The full dotted path from /tweak/state includes ancestors the finder
    // treats as waypoints; a stale head (window renamed) can still resolve
    // from the tail.
    if ids.len() > 1 {
        let found = tree.find_within(root, &ids[1..]);
        if !found.is_empty() {
            return Ok(found);
        }
        let tail = [*ids.last().unwrap()];
        let found = tree.find_within(root, &tail);
        if !found.is_empty() {
            return Ok(found);
        }
    }
    Err(format!("no widget at path {path:?}"))
}

/// The bare eval-apply: evaluate a splash chunk with the widget's own
/// `__script_source__` scope and apply it through the ordinary machinery.
/// No diff, no log — [`apply_splash_chunk`] wraps this for user-visible
/// edits; the tweaker's own scaffolding (body compression) uses it raw.
fn eval_chunk(cx: &mut Cx, widget: &WidgetRef, chunk: &str) -> Result<(), String> {
    let chunk = chunk.trim();
    let body = if chunk.starts_with('{') {
        chunk.to_string()
    } else {
        format!("{{{chunk}}}")
    };
    let code = format!("use mod.prelude.widgets.*\n__script_source__{body};");
    let errors = cx.with_vm(|vm| {
        // Install a captured-error sink so parse/apply problems come back to
        // the caller instead of only landing in the log.
        vm.bx.captured_errors = Some(Vec::new());
        let script_mod = ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: "tweak".to_string(),
            // One synthetic callsite: the body dedups by file/line/column and
            // resets itself whenever the code differs, so repeated applies
            // don't grow the body table.
            file: "tweak://apply".to_string(),
            line: 1,
            column: 1,
            code,
            values: Vec::new(),
        };
        let mut target = widget.clone();
        use crate::makepad_script::traits::ScriptApply;
        target.script_apply_eval(vm, script_mod);
        vm.take_errors()
    });
    if !errors.is_empty() {
        return Err(format!("splash error: {}", errors.join("; ")));
    }
    Ok(())
}

/// Apply one splash chunk onto a widget: eval + ordinary apply + full
/// relayout; the value diff (flat reflection before vs after) is logged and
/// returned.
pub fn apply_splash_chunk(
    cx: &mut Cx,
    widget: &WidgetRef,
    path: &str,
    chunk: &str,
    origin: &str,
) -> Result<Vec<TweakDiffEntry>, String> {
    let chunk = chunk.trim();
    let before = reflect_flat(cx, widget);
    eval_chunk(cx, widget, chunk)?;

    let after = reflect_flat(cx, widget);
    let now = cx.seconds_since_app_start();
    let mut changed = Vec::new();
    {
        let mut s = session().lock().unwrap();
        // The widget must be seen exactly as it renders while values move:
        // the solid outline yields to the faint stipple for a beat.
        s.suppress_until = now + SUPPRESS_LINGER;
        s.apply_gen += 1;
        // Sidebar/handle bursts (typing, scrubbing) log one line per pause,
        // not one per step; the diff records everything regardless.
        let interactive = origin != "remote";
        let should_log = |s: &mut TweakSession, prop: &str| -> bool {
            if !interactive {
                return true;
            }
            if let Some((last_path, last_prop, last_time)) = &s.last_sidebar_log {
                if last_path == path && last_prop == prop && now - *last_time < 1.0 {
                    return false;
                }
            }
            s.last_sidebar_log = Some((path.to_string(), prop.to_string(), now));
            true
        };
        for (name, new_value, _) in &after {
            let old_value = before
                .iter()
                .find(|(old_name, _, _)| old_name == name)
                .map(|(_, value, _)| value.clone())
                .unwrap_or_else(|| "-".to_string());
            if &old_value != new_value {
                s.next_seq += 1;
                let entry = TweakDiffEntry {
                    seq: s.next_seq,
                    path: path.to_string(),
                    prop: name.clone(),
                    old: old_value,
                    new: new_value.clone(),
                };
                if should_log(&mut s, &entry.prop) {
                    log!(
                        "TWEAK {} {} {} {} -> {}",
                        origin,
                        entry.path,
                        entry.prop,
                        entry.old,
                        entry.new
                    );
                }
                s.diff.push(entry.clone());
                changed.push(entry);
            }
        }
        if changed.is_empty() {
            // The chunk applied but the flat reflection saw no change. For a
            // dynamic shader input (border_radius: uniform(..)) the value
            // lives in the draw call, not a Rust field — a single-property
            // chunk still yields an honest (prop, old, new) entry: old is
            // the last applied value this session, else the reflected
            // default. Anything else logs as a raw chunk so nothing the
            // user did is silent.
            let entry = match single_prop_chunk(chunk) {
                Some((prop_name, value_text)) => {
                    let old = s
                        .diff
                        .iter()
                        .rev()
                        .find(|e| e.path == path && e.prop == prop_name)
                        .map(|e| e.new.clone())
                        .or_else(|| {
                            before
                                .iter()
                                .find(|(name, _, _)| *name == prop_name)
                                .map(|(_, value, _)| value.clone())
                        })
                        .unwrap_or_else(|| "-".to_string());
                    s.next_seq += 1;
                    TweakDiffEntry {
                        seq: s.next_seq,
                        path: path.to_string(),
                        prop: prop_name,
                        old,
                        new: value_text,
                    }
                }
                None => {
                    s.next_seq += 1;
                    TweakDiffEntry {
                        seq: s.next_seq,
                        path: path.to_string(),
                        prop: "(splash)".to_string(),
                        old: "-".to_string(),
                        new: chunk.to_string(),
                    }
                }
            };
            if should_log(&mut s, &entry.prop) {
                log!(
                    "TWEAK {} {} {} {} -> {}",
                    origin,
                    entry.path,
                    entry.prop,
                    entry.old,
                    entry.new
                );
            }
            s.diff.push(entry.clone());
            changed.push(entry);
        }
    }

    // Layout-affecting values (padding, margins, sizes) must take properly:
    // a FULL redraw re-runs every draw_walk, i.e. relayout, not just repaint.
    widget.redraw(cx);
    cx.redraw_all();
    Ok(changed)
}

// ---------------------------------------------------------------------------
// the remote surface — `Cx::tweak_callback`. Routes parse; this decides.
// ---------------------------------------------------------------------------

fn arg<'a>(args: &'a [(String, String)], keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        if let Some((_, value)) = args.iter().find(|(k, _)| k == key) {
            return Some(value.as_str());
        }
    }
    None
}

/// `"a.b: value"` (no braces, one property) — the shape every sidebar edit
/// and `prop=`/`value=` shorthand takes.
fn single_prop_chunk(chunk: &str) -> Option<(String, String)> {
    let chunk = chunk.trim();
    let inner = chunk.strip_prefix('{').map_or(chunk, |rest| rest.strip_suffix('}').unwrap_or(rest));
    let inner = inner.trim();
    if inner.contains('\n') || inner.contains('{') || inner.contains(',') {
        return None;
    }
    let (name, value) = inner.split_once(':')?;
    let name = name.trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
    {
        return None;
    }
    Some((name.to_string(), value.trim().to_string()))
}

fn fmt_f64(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1.0e15 {
        format!("{}", value as i64)
    } else {
        format!("{}", (value * 10000.0).round() / 10000.0)
    }
}

fn json_str(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 2);
    out.push('"');
    for c in input.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn pick_json(pick: &TweakPick) -> String {
    format!(
        "{{\"path\":{},\"ty\":{},\"r\":[{:.0},{:.0},{:.0},{:.0}],\"w\":{}{}}}",
        json_str(&pick.path),
        json_str(&pick.ty),
        pick.rect.pos.x,
        pick.rect.pos.y,
        pick.rect.size.x,
        pick.rect.size.y,
        pick.window_id,
        match &pick.band {
            Some(band) => format!(",\"band\":{}", json_str(band)),
            None => String::new(),
        }
    )
}

fn diff_json(entries: &[TweakDiffEntry]) -> String {
    let mut out = String::from("[");
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"path\":{},\"prop\":{},\"old\":{},\"new\":{}}}",
            json_str(&entry.path),
            json_str(&entry.prop),
            json_str(&entry.old),
            json_str(&entry.new)
        ));
    }
    out.push(']');
    out
}

fn strokes_json(strokes: &[TweakStroke]) -> String {
    let mut out = String::from("[");
    for (index, stroke) in strokes.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!("{{\"w\":{},\"widgets\":[", stroke.window_id));
        for (windex, path) in stroke.widgets.iter().enumerate() {
            if windex > 0 {
                out.push(',');
            }
            out.push_str(&json_str(path));
        }
        out.push_str("],\"points\":[");
        for (pindex, (x, y)) in stroke.points.iter().enumerate() {
            if pindex > 0 {
                out.push(',');
            }
            out.push_str(&format!("[{x:.0},{y:.0}]"));
        }
        out.push_str("]}");
    }
    out.push(']');
    out
}

/// Coalesce the diff log: per (path, prop) the FIRST old and the LAST new;
/// churn collapsed, no-ops dropped.
fn coalesce_diff(entries: &[TweakDiffEntry]) -> Vec<TweakDiffEntry> {
    let mut out: Vec<TweakDiffEntry> = Vec::new();
    for entry in entries {
        match out
            .iter_mut()
            .find(|e| e.path == entry.path && e.prop == entry.prop)
        {
            Some(existing) => {
                existing.new = entry.new.clone();
                existing.seq = entry.seq;
            }
            None => out.push(entry.clone()),
        }
    }
    out.retain(|entry| entry.old != entry.new);
    out
}

/// The `Cx::tweak_callback` the widgets crate registers in `set_ui_root`.
pub fn tweak_callback(
    cx: &mut Cx,
    op: &str,
    args: &[(String, String)],
) -> Result<String, String> {
    match op {
        "toggle" => {
            let on = match arg(args, &["on"]) {
                Some(value) => !matches!(value, "0" | "false" | "off" | "no"),
                None => !tweak_is_on(),
            };
            if let Some(annotate) = arg(args, &["annotate", "draw"]) {
                session().lock().unwrap().annotate =
                    !matches!(annotate, "0" | "false" | "off" | "no");
            }
            set_tweak_on(cx, on);
            let annotate = session().lock().unwrap().annotate;
            Ok(format!(
                "{{\"on\":{},\"annotate\":{}}}",
                if on { 1 } else { 0 },
                if annotate { 1 } else { 0 }
            ))
        }
        "state" => {
            let (pinned, hover, diff, strokes) = {
                let s = session().lock().unwrap();
                (
                    s.pinned.clone(),
                    s.hover.clone(),
                    s.diff.clone(),
                    s.strokes.clone(),
                )
            };
            let mut out = format!("{{\"on\":{}", if tweak_is_on() { 1 } else { 0 });
            if let Some(pick) = &pinned {
                out.push_str(",\"sel\":");
                out.push_str(&pick_json(pick));
                if let Ok(widget) = resolve_widget_by_path(cx, &pick.path) {
                    out.push_str(",\"props\":[");
                    for (index, (name, value, is_set)) in
                        reflect_flat(cx, &widget).into_iter().enumerate()
                    {
                        if index > 0 {
                            out.push(',');
                        }
                        out.push_str(&format!(
                            "{{\"n\":{},\"v\":{}{}}}",
                            json_str(&name),
                            json_str(&value),
                            if is_set { ",\"set\":1" } else { "" }
                        ));
                    }
                    out.push(']');
                    out.push_str(",\"cascade\":[");
                    for (index, lvl) in cascade_levels(cx, &widget).iter().enumerate() {
                        if index > 0 {
                            out.push(',');
                        }
                        out.push_str(&format!(
                            "{{\"loc\":{},\"file\":{},\"line\":{}",
                            json_str(&lvl.loc),
                            json_str(&lvl.file),
                            lvl.line
                        ));
                        if let Some(doc) = &lvl.doc {
                            out.push_str(&format!(",\"doc\":{}", json_str(doc)));
                        }
                        if !lvl.field_docs.is_empty() {
                            out.push_str(",\"fields\":{");
                            for (j, (field, doc)) in lvl.field_docs.iter().enumerate() {
                                if j > 0 {
                                    out.push(',');
                                }
                                out.push_str(&format!(
                                    "{}:{}",
                                    json_str(field),
                                    json_str(doc)
                                ));
                            }
                            out.push('}');
                        }
                        out.push_str(",\"sets\":[");
                        for (j, (name, _)) in lvl.sets.iter().enumerate() {
                            if j > 0 {
                                out.push(',');
                            }
                            out.push_str(&json_str(name));
                        }
                        out.push(']');
                        let over: Vec<&String> = lvl
                            .sets
                            .iter()
                            .filter(|(_, overridden)| *overridden)
                            .map(|(name, _)| name)
                            .collect();
                        if !over.is_empty() {
                            out.push_str(",\"over\":[");
                            for (j, name) in over.iter().enumerate() {
                                if j > 0 {
                                    out.push(',');
                                }
                                out.push_str(&json_str(name));
                            }
                            out.push(']');
                        }
                        out.push('}');
                    }
                    out.push(']');
                }
            }
            if let Some(pick) = &hover {
                out.push_str(",\"hover\":");
                out.push_str(&pick_json(pick));
            }
            out.push_str(",\"diff\":");
            out.push_str(&diff_json(&diff));
            out.push_str(",\"ann\":");
            out.push_str(&strokes_json(&strokes));
            out.push('}');
            Ok(out)
        }
        "apply" => {
            if !tweak_is_on() {
                set_tweak_on(cx, true);
            }
            let path = arg(args, &["path", "p"]).ok_or("need path=")?.to_string();
            let chunk = match arg(args, &["splash", "s", "chunk"]) {
                Some(chunk) => chunk.to_string(),
                None => {
                    let prop = arg(args, &["prop"]).ok_or("need splash= or prop=+value=")?;
                    let value = arg(args, &["value", "v"]).ok_or("need value=")?;
                    format!("{prop}: {value}")
                }
            };
            let widget = resolve_widget_by_path(cx, &path)?;
            // Applying to a widget selects it — the AI tweaks the very
            // instance the person would see outlined.
            let resolved_path = {
                let uid = widget.widget_uid();
                let ids = cx.widget_tree().path_to(uid);
                if ids.is_empty() {
                    path.clone()
                } else {
                    ids.iter()
                        .map(|id| live_id_token(*id))
                        .collect::<Vec<_>>()
                        .join(".")
                }
            };
            let changed = apply_splash_chunk(cx, &widget, &resolved_path, &chunk, "remote")?;
            {
                let mut s = session().lock().unwrap();
                let rect = widget.area().clipped_rect(cx);
                let ty = widget
                    .widget_type_id()
                    .and_then(|type_id| widget_type_names(cx).get(&type_id).copied())
                    .map(live_id_token)
                    .unwrap_or_else(|| "-".to_string());
                s.pinned = Some(TweakPick {
                    uid: widget.widget_uid().0,
                    path: resolved_path.clone(),
                    ty,
                    rect,
                    window_id: 0,
                    band: None,
                });
            }
            Ok(format!(
                "{{\"ok\":1,\"path\":{},\"changed\":{}}}",
                json_str(&resolved_path),
                diff_json(&changed)
            ))
        }
        "diff" => {
            let diff = session().lock().unwrap().diff.clone();
            Ok(format!("{{\"diff\":{}}}", diff_json(&diff)))
        }
        "clear" => {
            let mut s = session().lock().unwrap();
            s.diff.clear();
            s.strokes.clear();
            s.drew = false;
            s.apply_gen += 1;
            Ok("{\"ok\":1}".to_string())
        }
        "final" => {
            let (diff, strokes, drew) = {
                let s = session().lock().unwrap();
                (s.diff.clone(), s.strokes.clone(), s.drew)
            };
            let coalesced = coalesce_diff(&diff);
            Ok(format!(
                "{{\"final\":{},\"ann\":{},\"drew\":{}}}",
                diff_json(&coalesced),
                strokes_json(&strokes),
                if drew { 1 } else { 0 }
            ))
        }
        other => Err(format!("bad tweak op {other:?}")),
    }
}

// ---------------------------------------------------------------------------
// the Tweaker widget — hosted by Window, draws the overlay (outlines, pick
// label, annotation strokes) topmost in the window's own pass, so every
// ordinary grab already composites it.
// ---------------------------------------------------------------------------

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.View

    set_type_default() do #(DrawTweakOutline::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
            sdf.fill_keep(self.fill_color)
            let mut border = self.border_color
            if self.dash > 0.5 {
                // Stipple: dashes along the perimeter, in DEVICE pixels so
                // the pattern reads the same on any display.
                let p = self.pos * self.rect_size * self.dpi
                let t = modf(p.x + p.y, 8.0)
                if t > 4.0 {
                    border = vec4(0.0, 0.0, 0.0, 0.0)
                }
            }
            sdf.stroke(border, self.border_size)
            return sdf.result
        }
    }

    set_type_default() do #(DrawTweakStroke::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.circle(self.rect_size.x * 0.5, self.rect_size.y * 0.5, self.rect_size.x * 0.5)
            sdf.fill(self.stroke_color)
            return sdf.result
        }
    }

    mod.widgets.TweakerBase = #(Tweaker::register_widget(vm))
    mod.widgets.Tweaker = set_type_default() do mod.widgets.TweakerBase{
        width: 0
        height: 0
        draw_label +: {
            text_style +: {
                font_size: 7.5
            }
            color: #xffffff
        }
        draw_label_bg +: {
            color: #x1a2733dd
        }
        draw_splitter +: {
            color: #x161616
        }
        draw_panel_bg +: {
            color: #x303030
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTweakOutline {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub border_color: Vec4f,
    #[live]
    pub fill_color: Vec4f,
    #[live]
    pub border_size: f32,
    /// 1.0 = stippled (the tweaking-in-progress hairline).
    #[live]
    pub dash: f32,
    /// Device pixels per point, for dpi-true hairlines and dashes.
    #[live(1.0)]
    pub dpi: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTweakStroke {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub stroke_color: Vec4f,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Num,
    Bool,
    Text,
    Color,
    /// Read-only display row (the CASCADE section): label + value only.
    Info,
}

/// The sidebar's property groups, hottest first.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SectionKind {
    Layout,
    Style,
    Text,
    Behavior,
    Other,
    /// The construction chain of the selection: one block per prototype
    /// level (instance -> styled type -> base -> native), each with its
    /// source location, `///` docs and the keys it sets. CSS-inspector
    /// style: a key later marked ~name~ is overridden by a closer level.
    Cascade,
}

const SECTION_ORDER: [SectionKind; 6] = [
    SectionKind::Layout,
    SectionKind::Style,
    SectionKind::Text,
    SectionKind::Behavior,
    SectionKind::Other,
    SectionKind::Cascade,
];

impl SectionKind {
    fn index(self) -> usize {
        match self {
            SectionKind::Layout => 0,
            SectionKind::Style => 1,
            SectionKind::Text => 2,
            SectionKind::Behavior => 3,
            SectionKind::Other => 4,
            SectionKind::Cascade => 5,
        }
    }
    fn title(self) -> &'static str {
        match self {
            SectionKind::Layout => "LAYOUT",
            SectionKind::Style => "STYLE",
            SectionKind::Text => "TEXT",
            SectionKind::Behavior => "BEHAVIOR",
            SectionKind::Other => "OTHER",
            SectionKind::Cascade => "CASCADE",
        }
    }
}

/// Derive the group from what the reflection already knows — never a
/// hand-tagged schema. A known family matched by name prefix beats OTHER.
fn classify_prop(prop: &str, value: &str) -> SectionKind {
    let first = prop.split('.').next().unwrap_or("");
    match first {
        "width" | "height" | "abs_pos" | "margin" | "padding" | "spacing" | "line_spacing"
        | "align" | "flow" | "clip_x" | "clip_y" | "scroll" | "wrap_spacing" | "layout"
        | "metrics" => return SectionKind::Layout,
        "text" | "empty_text" | "label" | "title" | "suffix" => return SectionKind::Text,
        "visible" | "enabled" | "grab_key_focus" | "cursor" | "trigger_on_press"
        | "enable_long_press" | "reset_hover_on_click" | "block_signal_event"
        | "capture_overload" | "event_order" | "design_mode" | "skip_widget_tree_search"
        | "grab_focus" | "hover_actions_enabled" | "animator" => return SectionKind::Behavior,
        _ => {}
    }
    if first.ends_with("_walk") {
        // label_walk belongs with the text it lays out; the rest are layout.
        if first == "label_walk" {
            return SectionKind::Text;
        }
        return SectionKind::Layout;
    }
    if first.starts_with("draw_") || first == "text_style" || first == "icon" {
        return SectionKind::Style;
    }
    if value == "true" || value == "false" {
        return SectionKind::Behavior;
    }
    SectionKind::Other
}

/// Inset legs in the box-model order: left, top, right, bottom.
fn inset_leg_rank(leaf: &str) -> u32 {
    match leaf {
        "left" => 0,
        "top" => 1,
        "right" => 2,
        "bottom" => 3,
        _ => 4,
    }
}

/// The within-section ordering: (major, minor, alpha tail). Equal keys keep
/// reflection order (stable sort) — and changed rows never reorder at all.
fn section_rank(section: SectionKind, prop: &str) -> (u32, u32, String) {
    let mut parts = prop.split('.');
    let first = parts.next().unwrap_or("");
    let leaf = parts.next().unwrap_or("");
    match section {
        // Cascade rows are pushed in display order after the sort and keep
        // it via the stable sort's equal-key rule.
        SectionKind::Cascade => (0, 0, String::new()),
        SectionKind::Layout => {
            // The box-model progression, outside-in.
            let major = match first {
                "width" => 0,
                "height" => 1,
                "abs_pos" => 2,
                "margin" => 3,
                "padding" => 4,
                "spacing" => 5,
                "line_spacing" => 6,
                "align" => 7,
                "flow" => 8,
                "clip_x" => 9,
                "clip_y" => 10,
                "scroll" => 11,
                _ => 20,
            };
            let minor = match first {
                "margin" | "padding" => inset_leg_rank(leaf),
                "align" => match leaf {
                    "x" => 0,
                    "y" => 1,
                    _ => 2,
                },
                _ => 0,
            };
            (major, minor, prop.to_string())
        }
        SectionKind::Style => {
            // Surface first, then content layers; colors lead each family.
            let major = match first {
                "draw_bg" => 0,
                "draw_text" => 1,
                "draw_icon" => 2,
                _ => 3, // remaining draw_* families keep reflection order
            };
            let minor = if leaf == "color" {
                0
            } else if first == "draw_bg" {
                match leaf {
                    "border_color" => 2,
                    "border_size" => 3,
                    "border_radius" => 4,
                    _ if leaf.starts_with("color") => 1,
                    _ if leaf.starts_with("border_color") => 2,
                    _ => 10,
                }
            } else if first == "draw_text" {
                if leaf.starts_with("color") {
                    1
                } else if leaf == "text_style" || leaf.contains("font") {
                    2
                } else {
                    10
                }
            } else if first == "draw_icon" {
                if leaf.starts_with("color") {
                    1
                } else if leaf == "scale" {
                    2
                } else {
                    10
                }
            } else if leaf.starts_with("color") {
                1
            } else {
                10
            };
            (major, minor, prop.to_string())
        }
        SectionKind::Text => {
            let major = match first {
                "text" => 0,
                "empty_text" => 1,
                "label" => 2,
                "title" => 3,
                "suffix" => 4,
                "label_walk" => 5,
                "metrics" => 6,
                _ => 10,
            };
            (major, 0, prop.to_string())
        }
        SectionKind::Behavior => {
            let major = match first {
                "visible" => 0,
                "enabled" => 1,
                "grab_key_focus" | "grab_focus" => 2,
                "trigger_on_press" | "enable_long_press" | "reset_hover_on_click" => 3,
                "animator" => 4,
                _ => 10,
            };
            (major, 0, prop.to_string())
        }
        SectionKind::Other => (0, 0, prop.to_string()),
    }
}

/// One sidebar row: the property it edits and the uids of the field widgets
/// whose actions carry the edits.
#[derive(Clone)]
struct RowBinding {
    prop: String,
    kind: RowKind,
    value: String,
    /// The string value was quoted in reflection (a real string property).
    quoted: bool,
    section: SectionKind,
    /// Set at the instance level (own map) — the cascade cue: inherited
    /// values render with a dimmer label.
    set: bool,
    /// This property differs from its session-original (resettable).
    changed: bool,
    /// The session-original value (the first diff entry's `old`).
    original: Option<String>,
    field_uid: u64,
    /// The color row's swatch (hex box is `field_uid`).
    swatch_uid: u64,
}

/// One visible sidebar entry, with the rects the raw-pointer gestures
/// (section fold, label double-click reset) hit-test against.
/// The doc tooltip a hovered row shows (text + pointer position).
#[derive(Clone, PartialEq)]
struct HoverDoc {
    text: String,
    pos: Vec2d,
}

#[derive(Clone, Copy, PartialEq)]
enum BoxKind {
    Margin,
    Padding,
}

impl BoxKind {
    fn prop(self) -> &'static str {
        match self {
            BoxKind::Margin => "margin",
            BoxKind::Padding => "padding",
        }
    }
}

#[derive(Clone, Copy)]
enum VisKind {
    Section(SectionKind, usize, bool),
    Prop(usize),
    /// width + height compacted onto one row.
    Size,
    /// Four-sided box editor (mini rectangle, drag-to-scrub legs).
    BoxInset(BoxKind),
    /// spacing + flow on one row.
    FlowSpacing,
    /// The 9-dot align picker.
    AlignGrid,
    /// "show all (N)": the section's long tail, folded by default.
    More(SectionKind, usize),
}

#[derive(Clone)]
struct VisRow {
    kind: VisKind,
    /// The drawn row item; its area answers hit tests at gesture time
    /// (areas only become queryable after the frame commits).
    item: WidgetRef,
}

#[derive(Script, Widget)]
pub struct Tweaker {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[rust]
    overlay_list: Option<DrawList2d>,
    #[redraw]
    #[live]
    draw_outline: DrawTweakOutline,
    #[live]
    draw_stroke: DrawTweakStroke,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_label_bg: DrawColor,
    #[live]
    draw_splitter: DrawColor,
    /// Opaque backing for the whole sidebar band — the panel must read as
    /// one solid surface whatever the app's clear color is.
    #[live]
    draw_panel_bg: DrawColor,
    /// The property sidebar, built lazily at first open from a runtime
    /// splash chunk (every widget type is registered by then, whatever the
    /// registration order was).
    #[rust]
    sidebar: Option<WidgetRef>,
    #[rust]
    rows: Vec<RowBinding>,
    /// Selection uid the rows were last built for.
    #[rust]
    rows_uid: u64,
    /// The apply generation the rows were last built at.
    #[rust]
    rows_gen: u64,
    /// The visible entries (sections + rows after filter/fold), with the
    /// rects the raw-pointer gestures hit-test against. Rebuilt each draw.
    #[rust]
    visible: Vec<VisRow>,
    /// Live substring filter (the pinned search box), lowercase.
    #[rust]
    filter: String,
    /// Folded sections (ignored while the filter is active).
    #[rust]
    collapsed: [bool; 6],
    /// The search box's input uid, captured at draw.
    #[rust]
    search_uid: u64,
    /// Double-click detection on row labels: (time, row index).
    #[rust]
    last_label_click: Option<(f64, usize)>,
    /// While a text field is being live-edited: (row index, the value when
    /// focus began — Escape restores it).
    #[rust]
    text_edit_origin: Option<(usize, String)>,
    /// The radius-like style input of the selection, if any: the corner
    /// handles drive it. First of the direct-manipulation handle family.
    #[rust]
    radius_prop: Option<(String, f64)>,
    /// An in-flight corner-handle drag: (corner 0..3, start value, start pos).
    #[rust]
    radius_drag: Option<(usize, f64, Vec2d)>,
    /// Re-check the outline suppression window when it expires.
    #[rust]
    next_frame: NextFrame,
    /// Long-tail expansion per section ("show all (N)" clicked).
    #[rust]
    expanded: [bool; 6],
    /// Composite-row fields drawn this frame: (widget uid, prop it edits).
    /// Value/text changes from these uids apply like ordinary rows.
    #[rust]
    composite_fields: Vec<(u64, String)>,
    /// The 9 align dots drawn this frame: (uid, align.x, align.y).
    #[rust]
    composite_align: Vec<(u64, f64, f64)>,
    /// Segment buttons drawn this frame: (uid, splash chunk they apply).
    #[rust]
    composite_clicks: Vec<(u64, String)>,
    /// Doc-channel text per row prop (tooltips + scrubber hints).
    #[rust]
    row_docs: HashMap<String, String>,
    /// The doc tooltip shown on row hover.
    #[rust]
    hover_doc: Option<HoverDoc>,
    /// Which cascade level (0 = instance) set each top-level prop.
    #[rust]
    origin_levels: HashMap<String, usize>,
    /// How many cascade levels the selection has (colors + scroll target).
    #[rust]
    cascade_level_count: usize,
    /// Focus the filter input on the next sidebar draw ('/' or tweak-on).
    #[rust]
    focus_search_pending: bool,
    /// Previous tweak-mode state, to detect the on edge.
    #[rust]
    was_on: bool,
    /// The open color-picker popover's rect: input inside it belongs to
    /// the popup — row gestures skip it and the scroll list ignores wheel
    /// there (the input-side mirror of app < outlines < panel < popups).
    #[rust]
    open_popup: Option<Rect>,
    /// Linked-toggle state of the two box editors (margin, padding): a
    /// change to one leg applies to all four.
    #[rust]
    box_link: [bool; 2],
    /// The two link-toggle uids (margin, padding).
    #[rust]
    box_link_uids: [u64; 2],
    /// The panel's own overlay draw list: begun AFTER the outline overlay,
    /// so the stacking is app < outlines < panel < panel-popups — the app
    /// (dock tab bars included) can never read through the panel.
    #[rust]
    sidebar_list: Option<DrawList2d>,
    /// The sidebar band (splitter included), window-local, cached at draw.
    #[rust]
    band: Rect,
    /// This widget's window, learned at draw time.
    #[rust]
    my_window: Option<usize>,
    /// The margin-right currently applied to the window body.
    #[rust]
    applied_margin: f64,
    #[rust]
    splitter_drag: bool,
}

impl ScriptHook for Tweaker {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.overlay_list = Some(DrawList2d::script_new(vm));
        self.sidebar_list = Some(DrawList2d::script_new(vm));
    }
}

impl Tweaker {
    fn redraw_overlay(&mut self, cx: &mut Cx) {
        if let Some(overlay_list) = &self.overlay_list {
            overlay_list.redraw(cx);
        }
    }

    /// The window body is this widget's sibling in the window view; find it
    /// through the widget tree (zeros in the path are anonymous hops the
    /// finder treats as wildcards anyway, drop them).
    fn find_body(&self, cx: &Cx) -> Option<WidgetRef> {
        let tree = cx.widget_tree();
        let mut path = tree.path_to(self.uid);
        path.pop()?; // "tweaker"
        path.retain(|id| *id != LiveId(0));
        path.push(live_id!(body));
        let found = tree.find_within(tree.root_uid(), &path);
        if found.is_empty() {
            None
        } else {
            Some(found)
        }
    }

    /// Compress (or release) the app's UI: the body gets a right margin the
    /// size of the sidebar band, through the ordinary apply machinery so the
    /// relayout is the real one. Not a user edit — never enters the diff.
    fn ensure_body_margin(&mut self, cx: &mut Cx, desired: f64) {
        if (self.applied_margin - desired).abs() < 0.5 {
            return;
        }
        let Some(body) = self.find_body(cx) else {
            return;
        };
        let chunk = format!("margin.right: {desired:.0}");
        match eval_chunk(cx, &body, &chunk) {
            Ok(()) => {
                self.applied_margin = desired;
                cx.redraw_all();
            }
            Err(error) => {
                log!("TWEAK body compress failed: {error}");
                // Don't retry every frame.
                self.applied_margin = desired;
            }
        }
    }

    /// Build the sidebar widget from a runtime splash chunk, once (every
    /// widget type — the fab controls included — is registered by then).
    fn ensure_sidebar(&mut self, cx: &mut Cx) {
        if self.sidebar.is_some() {
            return;
        }
        let sidebar = cx.with_vm(|vm| {
            let value = script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View {
                    width: Fill
                    height: Fill
                    flow: Down
                    show_bg: true
                    draw_bg +: {
                        color: #x303030
                    }
                    padding: Inset{left: 4 right: 4 top: 6 bottom: 4}
                    spacing: 4
                    title_label := FabHeaderLabel {
                        width: Fill
                        margin: Inset{left: 4 top: 0 right: 0 bottom: 0}
                        text: "TWEAK"
                    }
                    path_label := FabLabelSmall {
                        width: Fill
                        margin: Inset{left: 4 top: 0 right: 0 bottom: 2}
                        text: "click a widget to inspect it"
                    }
                    search := FabSearch {}
                    props := PortalList {
                        width: Fill
                        height: Fill
                        margin: Inset{left: 0 top: 2 right: 0 bottom: 0}
                        // A desktop inspector full of draggable controls
                        // cannot share press-drags with its scroller:
                        // content drag-scroll is a touch idiom and it
                        // fought every field scrub for the gesture. Wheel
                        // and the scrollbar thumb still scroll.
                        drag_scrolling: false
                        SectionRow := FabSection {}
                        NumRow := FabPropRow {
                            value := FabValueInput {
                                width: Fill
                                height: 18
                            }
                            origin := FabLabelSmall { width: 12 margin: Inset{left: 2 top: 2 right: 0 bottom: 0} text: "" }
                        }
                        BoolRow := FabPropRow {
                            value := CheckBox {
                                width: Fit
                                height: Fit
                                text: ""
                            }
                            origin := FabLabelSmall { width: 12 margin: Inset{left: 2 top: 2 right: 0 bottom: 0} text: "" }
                        }
                        TextRow := FabPropRow {
                            value := TextInput {
                                width: Fill
                                height: 18
                                empty_text: ""
                                draw_bg +: {
                                    color: #x1d1d1d
                                    border_radius: 2.0
                                }
                                draw_text +: {
                                    ink_centered: true
                                    color: #xe6e6e6
                                    text_style +: {
                                        font_size: 8.5
                                    }
                                }
                            }
                            origin := FabLabelSmall { width: 12 margin: Inset{left: 2 top: 2 right: 0 bottom: 0} text: "" }
                        }
                        InfoRow := FabPropRow {
                            value := FabLabelSmall {
                                width: Fill
                                margin: Inset{left: 0 top: 2 right: 0 bottom: 0}
                                text: ""
                            }
                        }
                        SizeRow := FabPropRow {
                            height: Fit
                            size_col := View {
                                width: Fill
                                height: Fit
                                flow: Down
                                spacing: 2
                                w_row := View {
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    spacing: 4
                                    align: Align{x: 0.0 y: 0.5}
                                    w_axis := FabLabelSmall { width: 12 text: "W" }
                                    w_seg := View { width: Fit height: Fit flow: Right spacing: 1
                                        w_fill := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                        w_fit := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                        w_fix := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                    }
                                    w_input := TextInput {
                                        width: Fill
                                        height: 18
                                        empty_text: ""
                                        label_align: Align{x: 0.5 y: 0.5}
                                        draw_bg +: {
                                            color: #x1d1d1d
                                            border_radius: 2.0
                                        }
                                        draw_text +: {
                                            ink_centered: true
                                            color: #xe6e6e6
                                            text_style +: { font_size: 8.5 }
                                        }
                                    }
                                }
                                h_row := View {
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    spacing: 4
                                    align: Align{x: 0.0 y: 0.5}
                                    h_axis := FabLabelSmall { width: 12 text: "H" }
                                    h_seg := View { width: Fit height: Fit flow: Right spacing: 1
                                        h_fill := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                        h_fit := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                        h_fix := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                    }
                                    h_input := TextInput {
                                        width: Fill
                                        height: 18
                                        empty_text: ""
                                        label_align: Align{x: 0.5 y: 0.5}
                                        draw_bg +: {
                                            color: #x1d1d1d
                                            border_radius: 2.0
                                        }
                                        draw_text +: {
                                            ink_centered: true
                                            color: #xe6e6e6
                                            text_style +: { font_size: 8.5 }
                                        }
                                    }
                                }
                            }
                        }
                        BoxRow := FabPropRow {
                            height: Fit
                            margin: Inset{left: 0 right: 0 top: 3 bottom: 9}
                            box_col := View {
                                width: Fill
                                height: Fit
                                flow: Down
                                spacing: 2
                                top_row := View {
                                    width: Fill
                                    height: Fit
                                    align: Align{x: 0.5 y: 0.5}
                                    leg_top := FabValueInput { width: 64 height: 16 }
                                }
                                mid_row := View {
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    spacing: 4
                                    align: Align{x: 0.5 y: 0.5}
                                    leg_left := FabValueInput { width: 64 height: 16 }
                                    frame := View {
                                        width: Fill
                                        height: 20
                                        show_bg: true
                                        draw_bg +: {
                                            pixel: fn() {
                                                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                                                sdf.box(1.0, 1.0, self.rect_size.x - 2.0, self.rect_size.y - 2.0, 3.0)
                                                sdf.fill_keep(#x26262600)
                                                sdf.stroke(#x5a5a5a, 1.0)
                                                return sdf.result
                                            }
                                        }
                                    }
                                    leg_right := FabValueInput { width: 64 height: 16 }
                                }
                                bot_row := View {
                                    width: Fill
                                    height: Fit
                                    align: Align{x: 0.5 y: 0.5}
                                    leg_bottom := FabValueInput { width: 64 height: 16 }
                                }
                            }
                            link := CheckBox {
                                width: Fit
                                height: Fit
                                text: ""
                            }
                        }
                        FlowRow := FabPropRow {
                            spacing_input := FabValueInput {
                                width: 70
                                height: 18
                            }
                            flow_seg := View { width: Fit height: Fit flow: Right spacing: 1 margin: Inset{left: 6 right: 0 top: 0 bottom: 0}
                                f_right := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                f_down := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                f_over := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                f_wrap := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                            }
                            flow_field := FabLabelSmall {
                                width: Fill
                                margin: Inset{left: 4 top: 2 right: 0 bottom: 0}
                                text: ""
                            }
                        }
                        AlignRow := FabPropRow {
                            height: Fit
                            grid := View {
                                width: Fit
                                height: Fit
                                flow: Down
                                spacing: 2
                                row0 := View { width: Fit height: Fit flow: Right spacing: 2
                                    d0 := Button { width: 15 height: 15 text: "" padding: Inset{left:0 right:0 top:0 bottom:0} }
                                    d1 := Button { width: 15 height: 15 text: "" padding: Inset{left:0 right:0 top:0 bottom:0} }
                                    d2 := Button { width: 15 height: 15 text: "" padding: Inset{left:0 right:0 top:0 bottom:0} }
                                }
                                row1 := View { width: Fit height: Fit flow: Right spacing: 2
                                    d3 := Button { width: 15 height: 15 text: "" padding: Inset{left:0 right:0 top:0 bottom:0} }
                                    d4 := Button { width: 15 height: 15 text: "" padding: Inset{left:0 right:0 top:0 bottom:0} }
                                    d5 := Button { width: 15 height: 15 text: "" padding: Inset{left:0 right:0 top:0 bottom:0} }
                                }
                                row2 := View { width: Fit height: Fit flow: Right spacing: 2
                                    d6 := Button { width: 15 height: 15 text: "" padding: Inset{left:0 right:0 top:0 bottom:0} }
                                    d7 := Button { width: 15 height: 15 text: "" padding: Inset{left:0 right:0 top:0 bottom:0} }
                                    d8 := Button { width: 15 height: 15 text: "" padding: Inset{left:0 right:0 top:0 bottom:0} }
                                }
                            }
                            xy_label := FabLabelSmall {
                                width: Fill
                                margin: Inset{left: 8 top: 2 right: 0 bottom: 0}
                                text: ""
                            }
                        }
                        MoreRow := FabSection {}
                        ColorRow := FabPropRow {
                            value := TextInput {
                                width: Fill
                                height: 18
                                empty_text: "#rrggbbaa"
                                draw_bg +: {
                                    color: #x1d1d1d
                                    border_radius: 2.0
                                }
                                draw_text +: {
                                    ink_centered: true
                                    color: #xe6e6e6
                                    text_style +: {
                                        font_size: 8.5
                                    }
                                }
                            }
                            swatch := FabColorPick {
                                width: 28
                                height: 16
                            }
                            origin := FabLabelSmall { width: 12 margin: Inset{left: 2 top: 2 right: 0 bottom: 0} text: "" }
                        }
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        // Make the sidebar part of the widget tree (under this tweaker):
        // /snap lists its rows, so the same remote agent that watches the
        // session can drive the sidebar's fields too.
        cx.widget_tree_insert_child(self.uid, live_id!(sidebar), sidebar.clone());
        self.sidebar = Some(sidebar);
    }

    /// Rebuild the row bindings from the selection's reflected properties:
    /// classify into sections, order each section (box-model progression for
    /// layout, surface-then-content with colors leading for style), and mark
    /// what differs from its session-original (resettable).
    fn rebuild_rows(&mut self, cx: &mut Cx, sel_uid: u64, sel_path: &str) {
        let widget = cx.widget_tree().widget(WidgetUid(sel_uid));
        if widget.is_empty() {
            self.rows.clear();
            self.rows_uid = 0;
            self.radius_prop = None;
            return;
        }
        self.rows.clear();
        for (name, value, is_set) in reflect_flat(cx, &widget) {
            let (kind, display, quoted) = if value.starts_with('#') && parse_hex(&value).is_some()
            {
                (RowKind::Color, value, false)
            } else if value.parse::<f64>().is_ok() {
                (RowKind::Num, value, false)
            } else if value == "true" || value == "false" {
                (RowKind::Bool, value, false)
            } else if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
                (RowKind::Text, value[1..value.len() - 1].to_string(), true)
            } else {
                (RowKind::Text, value, false)
            };
            let section = classify_prop(&name, &display);
            self.rows.push(RowBinding {
                prop: name,
                kind,
                value: display,
                quoted,
                section,
                set: is_set,
                changed: false,
                original: None,
                field_uid: 0,
                swatch_uid: 0,
            });
        }
        // Session-original + resettable flags from the diff log.
        {
            let session = session().lock().unwrap();
            for row in &mut self.rows {
                let mut first_old: Option<&str> = None;
                let mut last_new: Option<&str> = None;
                for entry in session
                    .diff
                    .iter()
                    .filter(|e| e.path == sel_path && e.prop == row.prop)
                {
                    if first_old.is_none() {
                        first_old = Some(&entry.old);
                    }
                    last_new = Some(&entry.new);
                }
                if let (Some(old), Some(new)) = (first_old, last_new) {
                    row.original = Some(old.to_string());
                    row.changed = old != new;
                }
            }
        }
        // The concrete grouped ordering. Stable sort: equal keys keep
        // reflection order, changed rows never move.
        self.rows.sort_by(|a, b| {
            let ka = (a.section.index(), section_rank(a.section, &a.prop));
            let kb = (b.section.index(), section_rank(b.section, &b.prop));
            ka.cmp(&kb)
        });
        // The radius-like style input the corner handles drive: prefer
        // draw_bg's own border_radius, else the first *radius* style row.
        self.radius_prop = self
            .rows
            .iter()
            .find(|row| row.prop == "draw_bg.border_radius" && row.kind == RowKind::Num)
            .or_else(|| {
                self.rows.iter().find(|row| {
                    row.section == SectionKind::Style
                        && row.kind == RowKind::Num
                        && row.prop.split('.').next_back().unwrap_or("").contains("radius")
                        && !row.prop.contains("shadow")
                })
            })
            .and_then(|row| row.value.parse::<f64>().ok().map(|v| (row.prop.clone(), v)));
        // The CASCADE section: read-only rows, one block per construction
        // level, pushed after the sort so they keep exactly this order.
        {
            let widget = cx.widget_tree().widget(WidgetUid(sel_uid));
            self.row_docs = collect_row_docs(cx, &widget);
            self.origin_levels.clear();
            let mut push = |prop: String, value: String| {
                self.rows.push(RowBinding {
                    prop,
                    kind: RowKind::Info,
                    value,
                    quoted: false,
                    section: SectionKind::Cascade,
                    set: true,
                    changed: false,
                    original: None,
                    field_uid: 0,
                    swatch_uid: 0,
                });
            };
            let levels = cascade_levels(cx, &widget);
            self.cascade_level_count = levels.len();
            for (i, lvl) in levels.iter().enumerate() {
                for (key, overridden) in &lvl.sets {
                    if !overridden {
                        self.origin_levels.entry(key.clone()).or_insert(i);
                    }
                }
                let mut doc_lines = lvl.doc.as_deref().unwrap_or("").lines();
                push(
                    format!("L{i} {}", lvl.loc),
                    doc_lines.next().unwrap_or("").to_string(),
                );
                for line in doc_lines {
                    push("  \u{b7}".to_string(), line.to_string());
                }
                for (field, doc) in &lvl.field_docs {
                    for (j, line) in doc.lines().enumerate() {
                        let prop = if j == 0 {
                            format!("  {field}")
                        } else {
                            "  \u{b7}".to_string()
                        };
                        push(prop, line.to_string());
                    }
                }
                if !lvl.sets.is_empty() {
                    // ~name~ = overridden by a closer level (struck through
                    // in spirit; the row font has no strike style yet).
                    let joined = lvl
                        .sets
                        .iter()
                        .map(|(name, overridden)| {
                            if *overridden {
                                format!("~{name}~")
                            } else {
                                name.clone()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    // chunk long lists into readable rows
                    let mut line = String::new();
                    let mut first = true;
                    for word in joined.split(' ') {
                        if !line.is_empty() && line.len() + word.len() > 34 {
                            push(
                                if first { "  sets".to_string() } else { "  \u{b7}".to_string() },
                                line.clone(),
                            );
                            first = false;
                            line.clear();
                        }
                        if !line.is_empty() {
                            line.push(' ');
                        }
                        line.push_str(word);
                    }
                    if !line.is_empty() {
                        push(
                            if first { "  sets".to_string() } else { "  \u{b7}".to_string() },
                            line,
                        );
                    }
                }
            }
        }
        self.rows_uid = sel_uid;
    }

    /// True when a row is folded into one of LAYOUT's composite rows
    /// (size pair, box editors, spacing/flow, align grid).
    fn layout_composited(prop: &str) -> bool {
        let first = prop.split('.').next().unwrap_or("");
        matches!(
            first,
            "width" | "height" | "margin" | "padding" | "spacing" | "flow" | "align"
        )
    }

    /// The curated STYLE row set: colors and the handful of numbers a
    /// designer actually reaches for; the long tail folds behind
    /// "show all (N)".
    fn style_curated(row: &RowBinding) -> bool {
        if row.kind == RowKind::Color {
            return true;
        }
        let leaf = row.prop.split('.').next_back().unwrap_or("");
        leaf.contains("radius")
            || leaf.contains("border_size")
            || leaf.contains("font_size")
            || leaf.contains("shadow")
    }

    fn row_index(&self, prop: &str) -> Option<usize> {
        self.rows.iter().position(|row| row.prop == prop)
    }

    fn row_value(&self, prop: &str) -> Option<&str> {
        self.rows
            .iter()
            .find(|row| row.prop == prop)
            .map(|row| row.value.as_str())
    }

    /// A row matches the filter on its name OR its value/doc text (the
    /// cascade rows carry annotation text in their values, so docs and
    /// friendly names are searchable too).
    fn row_matches_filter(&self, row: &RowBinding) -> bool {
        self.filter.is_empty()
            || row.prop.to_lowercase().contains(&self.filter)
            || row.value.to_lowercase().contains(&self.filter)
    }

    /// The visible entry list: sections in order, folded sections
    /// collapsed, LAYOUT compacted into the composite row grammar (size
    /// pair, box editors, spacing/flow, align grid), the long tails behind
    /// "show all (N)". The filter searches across all sections at once on
    /// names, values and annotation text; while filtering, curation
    /// suspends (a long-tail match shows regardless) and sections stay
    /// open.
    fn build_visible(&self) -> Vec<VisKind> {
        let filtering = !self.filter.is_empty();
        let mut out = Vec::new();
        for section in SECTION_ORDER {
            let members: Vec<usize> = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, row)| row.section == section && self.row_matches_filter(row))
                .map(|(index, _)| index)
                .collect();
            let composites: Vec<VisKind> = if section == SectionKind::Layout && !filtering {
                let mut list = Vec::new();
                // Always present: an axis with no reflected row IS the Fit
                // state — the segments must still show it.
                list.push(VisKind::Size);
                if self.rows.iter().any(|r| r.prop.starts_with("margin")) {
                    list.push(VisKind::BoxInset(BoxKind::Margin));
                }
                if self.rows.iter().any(|r| r.prop.starts_with("padding")) {
                    list.push(VisKind::BoxInset(BoxKind::Padding));
                }
                if self.row_index("spacing").is_some() || self.row_index("flow").is_some() {
                    list.push(VisKind::FlowSpacing);
                }
                if self.row_index("align.x").is_some() {
                    list.push(VisKind::AlignGrid);
                }
                list
            } else {
                Vec::new()
            };
            if members.is_empty() && composites.is_empty() {
                continue;
            }
            let open = filtering || !self.collapsed[section.index()];
            out.push(VisKind::Section(section, members.len(), open));
            if !open {
                continue;
            }
            out.extend(composites.iter().copied());
            let expanded = filtering || self.expanded[section.index()];
            let mut hidden = 0usize;
            for index in members {
                let row = &self.rows[index];
                let in_tail = match section {
                    SectionKind::Layout => {
                        !filtering && Self::layout_composited(&row.prop)
                            || (!expanded && !Self::layout_composited(&row.prop))
                    }
                    SectionKind::Style => !expanded && !Self::style_curated(row),
                    _ => false,
                };
                if !filtering && in_tail {
                    // Rows folded into composites never re-appear; the
                    // rest count toward the expander.
                    if !(section == SectionKind::Layout && Self::layout_composited(&row.prop))
                        && !(section == SectionKind::Style && Self::style_curated(row))
                    {
                        hidden += 1;
                    }
                    continue;
                }
                out.push(VisKind::Prop(index));
            }
            if hidden > 0 {
                out.push(VisKind::More(section, hidden));
            }
        }
        out
    }

    /// Draw the opaque panel backing, the splitter, and the property
    /// sidebar into the vacated band.
    fn draw_sidebar(&mut self, cx: &mut Cx2d, scope: &mut Scope, sel: Option<&TweakPick>) {
        let pass_size = cx.current_pass_size();
        let width = sidebar_width();
        // The band owns its full column, window-top to bottom: the app (tab
        // bars, chrome, anything) must never read through the panel.
        let band = Rect {
            pos: dvec2(pass_size.x - width, 0.0),
            size: dvec2(width, pass_size.y),
        };
        self.band = band;

        // One solid surface first: the band must never show the app's bare
        // clear color between rows.
        self.draw_panel_bg.draw_abs(cx, band);
        self.draw_splitter.draw_abs(
            cx,
            Rect {
                pos: band.pos,
                size: dvec2(SPLITTER_WIDTH, band.size.y),
            },
        );

        self.ensure_sidebar(cx);
        let sidebar = self.sidebar.as_ref().unwrap().clone();

        match sel {
            Some(sel) => {
                sidebar
                    .child(live_id!(title_label))
                    .set_text(cx, &format!("{}  \u{2022}  {} props", sel.ty, self.rows.len()));
                sidebar.child(live_id!(path_label)).set_text(cx, &sel.path);
            }
            None => {
                sidebar.child(live_id!(title_label)).set_text(cx, "TWEAK");
                sidebar
                    .child(live_id!(path_label))
                    .set_text(cx, "click a widget to inspect it");
            }
        }
        self.search_uid = sidebar
            .child(live_id!(search))
            .child(live_id!(input))
            .widget_uid()
            .0;
        if self.focus_search_pending {
            let input = sidebar.child(live_id!(search)).child(live_id!(input));
            if input.area() != Area::Empty {
                cx.set_key_focus(input.area());
                self.focus_search_pending = false;
            }
        }

        self.composite_fields.clear();
        self.composite_align.clear();
        self.composite_clicks.clear();
        self.open_popup = None;
        let entries = self.build_visible();
        let mut visible_rects: Vec<VisRow> = Vec::with_capacity(entries.len());

        let walk = Walk::abs_rect(Rect {
            pos: dvec2(band.pos.x + SPLITTER_WIDTH, band.pos.y),
            size: dvec2(band.size.x - SPLITTER_WIDTH, band.size.y),
        });
        while let Some(step_widget) = sidebar.draw_walk(cx, scope, walk).step() {
            let Some(mut list) = step_widget.borrow_mut::<PortalList>() else {
                continue;
            };
            list.set_item_range(cx, 0, entries.len());
            while let Some(entry_id) = list.next_visible_item(cx) {
                if entry_id >= entries.len() {
                    continue;
                }
                let entry = entries[entry_id];
                let template = match entry {
                    VisKind::Section(..) => live_id!(SectionRow),
                    VisKind::More(..) => live_id!(MoreRow),
                    VisKind::Size => live_id!(SizeRow),
                    VisKind::BoxInset(_) => live_id!(BoxRow),
                    VisKind::FlowSpacing => live_id!(FlowRow),
                    VisKind::AlignGrid => live_id!(AlignRow),
                    VisKind::Prop(index) => match self.rows[index].kind {
                        RowKind::Num => live_id!(NumRow),
                        RowKind::Bool => live_id!(BoolRow),
                        RowKind::Color => live_id!(ColorRow),
                        RowKind::Text => live_id!(TextRow),
                        RowKind::Info => live_id!(InfoRow),
                    },
                };
                let (item, existed) = list.item_with_existed(cx, entry_id, template);
                if item.is_empty() {
                    continue;
                }
                match entry {
                    VisKind::Section(section, count, open) => {
                        item.child(live_id!(title)).set_text(
                            cx,
                            &format!(
                                "{} {} ({count})",
                                if open { "-" } else { "+" },
                                section.title()
                            ),
                        );
                    }
                    VisKind::More(_, count) => {
                        item.child(live_id!(title))
                            .set_text(cx, &format!("\u{2026} show all ({count})"));
                    }
                    VisKind::Size => {
                        item.child(live_id!(name)).set_text(cx, "size");
                        let size_col = item.child(live_id!(size_col));
                        for (axis, row_id, seg_id, input_id, segs) in [
                            (
                                "width",
                                live_id!(w_row),
                                live_id!(w_seg),
                                live_id!(w_input),
                                [live_id!(w_fill), live_id!(w_fit), live_id!(w_fix)],
                            ),
                            (
                                "height",
                                live_id!(h_row),
                                live_id!(h_seg),
                                live_id!(h_input),
                                [live_id!(h_fill), live_id!(h_fit), live_id!(h_fix)],
                            ),
                        ] {
                            let row = size_col.child(row_id);
                            let fixed = self
                                .row_value(axis)
                                .and_then(|v| v.parse::<f64>().ok());
                            let filled =
                                self.row_index(&format!("{axis}.weight")).is_some();
                            // The autolayout convention: Fill spreads
                            // (arrows out), Fit hugs (arrows in), Fixed is
                            // a number — the value field lights only then.
                            let seg = row.child(seg_id);
                            let labels = ["\u{2194}", "\u{2192}\u{2190}", "#"];
                            let active = if fixed.is_some() {
                                2
                            } else if filled {
                                0
                            } else {
                                1
                            };
                            for (i, seg_child) in segs.into_iter().enumerate() {
                                let btn = seg.child(seg_child);
                                btn.set_text(
                                    cx,
                                    &if i == active {
                                        format!("[{}]", labels[i])
                                    } else {
                                        labels[i].to_string()
                                    },
                                );
                                let chunk = match i {
                                    0 => format!("{axis}: Fill"),
                                    1 => format!("{axis}: Fit"),
                                    _ => format!(
                                        "{axis}: {}",
                                        fixed.map(fmt_f64).unwrap_or_else(|| "100".into())
                                    ),
                                };
                                self.composite_clicks.push((btn.widget_uid().0, chunk));
                            }
                            let input = row.child(input_id);
                            if input.area() == Area::Empty || !cx.has_key_focus(input.area())
                            {
                                let text = match fixed {
                                    Some(v) => fmt_f64(v),
                                    None if filled => "Fill".to_string(),
                                    None => "Fit".to_string(),
                                };
                                input.set_text(cx, &text);
                            }
                            self.composite_fields
                                .push((input.widget_uid().0, axis.to_string()));
                        }
                    }
                    VisKind::BoxInset(kind) => {
                        let base = kind.prop();
                        item.child(live_id!(name)).set_text(cx, base);
                        let all = self
                            .row_value(base)
                            .and_then(|v| v.parse::<f64>().ok());
                        let box_col = item.child(live_id!(box_col));
                        let legs = [
                            (live_id!(mid_row), live_id!(leg_left), "left"),
                            (live_id!(top_row), live_id!(leg_top), "top"),
                            (live_id!(mid_row), live_id!(leg_right), "right"),
                            (live_id!(bot_row), live_id!(leg_bottom), "bottom"),
                        ];
                        for (row, child, leg) in legs {
                            let prop = format!("{base}.{leg}");
                            let value = self
                                .row_value(&prop)
                                .and_then(|v| v.parse::<f64>().ok())
                                .or(all)
                                .unwrap_or(0.0);
                            let field = box_col.child(row).child(child);
                            if let Some(mut input) = field.borrow_mut::<FabValueInput>() {
                                input.set_value(cx, value);
                            }
                            self.composite_fields.push((field.widget_uid().0, prop));
                        }
                        let link = item.child(live_id!(link));
                        let link_index = (kind == BoxKind::Padding) as usize;
                        if let Some(mut check) = link.borrow_mut::<CheckBox>() {
                            check.set_active(cx, self.box_link[link_index], Animate::No);
                        }
                        self.box_link_uids[link_index] = link.widget_uid().0;
                    }
                    VisKind::FlowSpacing => {
                        item.child(live_id!(name)).set_text(cx, "spacing");
                        let field = item.child(live_id!(spacing_input));
                        if let Some(mut input) = field.borrow_mut::<FabValueInput>() {
                            let v = self
                                .row_value("spacing")
                                .and_then(|v| v.parse::<f64>().ok())
                                .unwrap_or(0.0);
                            input.set_value(cx, v);
                        }
                        self.composite_fields
                            .push((field.widget_uid().0, "spacing".into()));
                        let seg = item.child(live_id!(flow_seg));
                        let flows = [
                            (live_id!(f_right), "R", "flow: Right"),
                            (live_id!(f_down), "D", "flow: Down"),
                            (live_id!(f_over), "O", "flow: Overlay"),
                            (live_id!(f_wrap), "W", "flow: RightWrap"),
                        ];
                        for (child, label, chunk) in flows {
                            let btn = seg.child(child);
                            btn.set_text(cx, label);
                            self.composite_clicks
                                .push((btn.widget_uid().0, chunk.to_string()));
                        }
                        let flow_text = self
                            .row_value("flow")
                            .map(|v| format!("flow {v}"))
                            .unwrap_or_default();
                        item.child(live_id!(flow_field)).set_text(cx, &flow_text);
                    }
                    VisKind::AlignGrid => {
                        item.child(live_id!(name)).set_text(cx, "align");
                        let ax = self
                            .row_value("align.x")
                            .and_then(|v| v.parse::<f64>().ok())
                            .unwrap_or(0.0);
                        let ay = self
                            .row_value("align.y")
                            .and_then(|v| v.parse::<f64>().ok())
                            .unwrap_or(0.0);
                        let dots = [
                            live_id!(d0), live_id!(d1), live_id!(d2),
                            live_id!(d3), live_id!(d4), live_id!(d5),
                            live_id!(d6), live_id!(d7), live_id!(d8),
                        ];
                        for (i, dot_id) in dots.into_iter().enumerate() {
                            let dx = (i % 3) as f64 * 0.5;
                            let dy = (i / 3) as f64 * 0.5;
                            let dot = item
                                .child(live_id!(grid))
                                .child(match i / 3 {
                                    0 => live_id!(row0),
                                    1 => live_id!(row1),
                                    _ => live_id!(row2),
                                })
                                .child(dot_id);
                            let on = (ax - dx).abs() < 0.25 && (ay - dy).abs() < 0.25;
                            dot.set_text(cx, if on { "\u{2022}" } else { "" });
                            self.composite_align.push((dot.widget_uid().0, dx, dy));
                        }
                        item.child(live_id!(xy_label))
                            .set_text(cx, &format!("x {ax:.2}  y {ay:.2}"));
                    }
                    VisKind::Prop(index) => {
                        let name = item.child(live_id!(name));
                        name.set_text(cx, &self.rows[index].prop);
                        let _ = &name;
                        // The changed-indicator: a resettable row's label
                        // reads brighter (double-click it to reset).
                        // Cascade rows: the level header takes its level's
                        // color (the same palette the origin dots use).
                        let cascade_level = if self.rows[index].section == SectionKind::Cascade {
                            let prop = &self.rows[index].prop;
                            prop.strip_prefix('L')
                                .and_then(|rest| rest.split(' ').next())
                                .and_then(|n| n.parse::<usize>().ok())
                        } else {
                            None
                        };
                        if let Some(mut label) = name.borrow_mut::<Label>() {
                            label.draw_text.color = if let Some(level) = cascade_level {
                                Self::level_color(level)
                            } else if self.rows[index].changed {
                                vec4(1.0, 0.78, 0.42, 1.0)
                            } else if self.rows[index].set {
                                // set at the instance level
                                vec4(0.72, 0.72, 0.72, 1.0)
                            } else {
                                // inherited from a prototype level
                                vec4(0.48, 0.48, 0.48, 1.0)
                            };
                        }
                        // The origin dot: colored by the closest proto
                        // level that sets this prop; click opens the
                        // cascade scrolled to that level.
                        let origin = item.child(live_id!(origin));
                        if !origin.is_empty() {
                            let first = self.rows[index]
                                .prop
                                .split('.')
                                .next()
                                .unwrap_or("")
                                .to_string();
                            match self.origin_levels.get(&first) {
                                Some(&level) => {
                                    origin.set_text(cx, "\u{25cf}");
                                    if let Some(mut label) = origin.borrow_mut::<Label>() {
                                        label.draw_text.color = Self::level_color(level);
                                    }
                                }
                                None => origin.set_text(cx, ""),
                            }
                        }
                        let field = item.child(live_id!(value));
                        self.rows[index].field_uid = field.widget_uid().0;
                        match self.rows[index].kind {
                            RowKind::Num => {
                                if let Some(mut input) = field.borrow_mut::<FabValueInput>() {
                                    // The annotation channel drives the
                                    // scrubber: `/**name 0..24 step 0.5*/`
                                    // becomes bounds + granularity.
                                    if let Some(doc) = self.row_docs.get(&self.rows[index].prop)
                                    {
                                        let hint = parse_doc_hint(doc);
                                        input.set_hint(hint.min, hint.max, hint.step);
                                    }
                                    if let Ok(v) = self.rows[index].value.parse::<f64>() {
                                        input.set_value(cx, v);
                                    }
                                }
                            }
                            RowKind::Bool => {
                                if let Some(mut check) = field.borrow_mut::<CheckBox>() {
                                    check.set_active(
                                        cx,
                                        self.rows[index].value == "true",
                                        Animate::No,
                                    );
                                }
                            }
                            RowKind::Text => {
                                let editing = self
                                    .text_edit_origin
                                    .as_ref()
                                    .is_some_and(|(row, _)| *row == index);
                                if !editing
                                    && (!existed
                                        || field.area() == Area::Empty
                                        || !cx.has_key_focus(field.area()))
                                {
                                    field.set_text(cx, &self.rows[index].value);
                                }
                            }
                            RowKind::Info => {
                                field.set_text(cx, &self.rows[index].value);
                            }
                            RowKind::Color => {
                                if field.area() == Area::Empty
                                    || !cx.has_key_focus(field.area())
                                {
                                    field.set_text(cx, &self.rows[index].value);
                                }
                                let swatch = item.child(live_id!(swatch));
                                self.rows[index].swatch_uid = swatch.widget_uid().0;
                                let rgba = parse_hex(&self.rows[index].value);
                                {
                                    if let Some(mut pick) =
                                        swatch.borrow_mut::<FabColorPick>()
                                    {
                                        if !pick.is_open() {
                                            if let Some((rgba, _)) = rgba {
                                                pick.set_rgba(cx, rgba);
                                            }
                                        } else {
                                            let rect = pick.popover_rect();
                                            if rect.size.x > 0.0 {
                                                self.open_popup = Some(rect);
                                            }
                                        }
                                    }
                                }
                                drop(swatch);
                            }
                        }
                    }
                }
                item.draw_all(cx, &mut Scope::empty());
                visible_rects.push(VisRow {
                    kind: entry,
                    item: item.clone(),
                });
            }
        }
        self.visible = visible_rects;
        // The doc tooltip chip: annotation text for the hovered row,
        // clamped into the band.
        if let Some(hover) = self.hover_doc.clone() {
            let label_height = 16.0;
            let approx = (hover.text.chars().count() as f64) * 5.4 + 10.0;
            let mut pos = hover.pos;
            pos.x = pos
                .x
                .clamp(band.pos.x, (band.pos.x + band.size.x - approx).max(band.pos.x));
            pos.y = pos.y.max(0.0);
            self.draw_label_bg.draw_abs(
                cx,
                Rect {
                    pos,
                    size: dvec2(approx, label_height),
                },
            );
            self.draw_label
                .draw_abs(cx, pos + dvec2(5.0, 2.0), &hover.text);
        }
    }

    /// Apply one sidebar-originated chunk to the selection (same path the
    /// AI uses, same diff log; TWEAK log lines throttle to one per pause).
    fn sidebar_apply(&mut self, cx: &mut Cx, sel: &TweakPick, chunk: &str) {
        let widget = cx.widget_tree().widget(WidgetUid(sel.uid));
        if widget.is_empty() {
            return;
        }
        if let Err(error) = apply_splash_chunk(cx, &widget, &sel.path, chunk, "sidebar") {
            log!("TWEAK sidebar apply failed: {error}");
        }
        self.rows_uid = 0;
    }

    /// Reset one property to its session-original value: apply it back
    /// through the same machinery and REMOVE its diff entries — a reset
    /// property is untouched again and /tweak/final never mentions it.
    fn reset_row(&mut self, cx: &mut Cx, sel: &TweakPick, row_index: usize) {
        let Some(row) = self.rows.get(row_index) else {
            return;
        };
        let Some(original) = row.original.clone() else {
            return; // nothing to reset
        };
        let prop = row.prop.clone();
        let widget = cx.widget_tree().widget(WidgetUid(sel.uid));
        if widget.is_empty() {
            return;
        }
        let chunk = format!("{prop}: {original}");
        match eval_chunk(cx, &widget, &chunk) {
            Ok(()) => {
                let mut session = session().lock().unwrap();
                session
                    .diff
                    .retain(|entry| !(entry.path == sel.path && entry.prop == prop));
                session.suppress_until = cx.seconds_since_app_start() + SUPPRESS_LINGER;
                session.apply_gen += 1;
                drop(session);
                log!("TWEAK reset {} {} -> {}", sel.path, prop, original);
                self.rows_uid = 0;
                widget.redraw(cx);
                cx.redraw_all();
            }
            Err(error) => {
                log!("TWEAK reset failed for {} {}: {}", sel.path, prop, error);
            }
        }
    }

    /// Sidebar edits: every field action becomes one splash chunk applied
    /// to the selected instance — textbox, swatch, picker and the AI's
    /// /tweak/apply all land in the same diff entry per property.
    fn handle_sidebar_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let sel = session().lock().unwrap().pinned.clone();
        // The search box filters even with nothing selected.
        for action in actions {
            let Some(widget_action) = action.as_widget_action() else {
                continue;
            };
            if self.search_uid != 0 && widget_action.widget_uid.0 == self.search_uid {
                match widget_action.cast::<TextInputAction>() {
                    TextInputAction::Changed(text) => {
                        self.filter = text.to_lowercase();
                        self.redraw_sidebar(cx);
                    }
                    TextInputAction::Escaped => {
                        self.filter.clear();
                        if let Some(sidebar) = self.sidebar.as_ref() {
                            sidebar
                                .child(live_id!(search))
                                .child(live_id!(input))
                                .set_text(cx, "");
                        }
                        cx.set_key_focus(Area::Empty);
                        self.redraw_sidebar(cx);
                    }
                    _ => {}
                }
            }
        }
        let Some(sel) = sel else {
            return;
        };
        #[derive(Clone)]
        enum Edit {
            Apply(String),
            Revert(usize, String),
            HoldOn(usize),
            HoldOff,
        }
        let mut edits: Vec<Edit> = Vec::new();
        for action in actions {
            let Some(widget_action) = action.as_widget_action() else {
                continue;
            };
            let action_uid = widget_action.widget_uid.0;
            // Composite-row fields (size pair, box legs, spacing, align
            // dots, link toggles) are not rows; resolve them first.
            if let Some((_, prop)) = self
                .composite_fields
                .iter()
                .find(|(uid, _)| *uid == action_uid)
                .cloned()
            {
                match widget_action.cast::<FabValueInputAction>() {
                    FabValueInputAction::Changed(v) => {
                        // Linked box editor: one leg drives all four.
                        let link_index = if prop.starts_with("margin.") {
                            Some(0)
                        } else if prop.starts_with("padding.") {
                            Some(1)
                        } else {
                            None
                        };
                        match link_index {
                            Some(li) if self.box_link[li] => {
                                let base = prop.split('.').next().unwrap_or("");
                                edits.push(Edit::Apply(format!(
                                    "{base}: Inset{{left: {v} top: {v} right: {v} bottom: {v}}}",
                                    v = fmt_f64(v)
                                )));
                            }
                            _ => edits.push(Edit::Apply(format!("{prop}: {}", fmt_f64(v)))),
                        }
                        continue;
                    }
                    FabValueInputAction::Ended(_) => {
                        edits.push(Edit::HoldOff);
                        continue;
                    }
                    _ => {}
                }
                match widget_action.cast::<TextInputAction>() {
                    TextInputAction::Changed(text) => {
                        let text = text.trim().to_string();
                        if !text.is_empty() {
                            edits.push(Edit::Apply(format!("{prop}: {text}")));
                        }
                        continue;
                    }
                    _ => {}
                }
                continue;
            }
            if let Some((_, chunk)) = self
                .composite_clicks
                .iter()
                .find(|(uid, _)| *uid == action_uid)
                .cloned()
            {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    edits.push(Edit::Apply(chunk));
                }
                continue;
            }
            if let Some(&(_, ax, ay)) = self
                .composite_align
                .iter()
                .find(|(uid, _, _)| *uid == action_uid)
            {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    edits.push(Edit::Apply(format!("align: Align{{x: {ax} y: {ay}}}")));
                }
                continue;
            }
            if let Some(link_index) = self.box_link_uids.iter().position(|uid| *uid == action_uid)
            {
                if let CheckBoxAction::Change(v) = widget_action.cast::<CheckBoxAction>() {
                    self.box_link[link_index] = v;
                }
                continue;
            }
            let Some((index, binding)) = self
                .rows
                .iter()
                .enumerate()
                .find(|(_, b)| {
                    (b.field_uid != 0 && b.field_uid == action_uid)
                        || (b.swatch_uid != 0 && b.swatch_uid == action_uid)
                })
                .map(|(i, b)| (i, b.clone()))
            else {
                continue;
            };
            let is_swatch = binding.swatch_uid != 0 && binding.swatch_uid == action_uid;
            match binding.kind {
                // CASCADE rows are read-only labels; nothing to apply.
                RowKind::Info => {}
                RowKind::Num => match widget_action.cast::<FabValueInputAction>() {
                    FabValueInputAction::Changed(v) => {
                        edits.push(Edit::Apply(format!("{}: {}", binding.prop, fmt_f64(v))));
                    }
                    FabValueInputAction::Ended(_) => {
                        edits.push(Edit::HoldOff);
                    }
                    _ => {}
                },
                RowKind::Bool => match widget_action.cast::<CheckBoxAction>() {
                    CheckBoxAction::Change(v) => {
                        edits.push(Edit::Apply(format!("{}: {}", binding.prop, v)));
                    }
                    _ => {}
                },
                RowKind::Text => match widget_action.cast::<TextInputAction>() {
                    // Text applies LIVE as you type; Enter/blur only end the
                    // interaction, Escape restores the focus-start value.
                    TextInputAction::KeyFocus => {
                        edits.push(Edit::HoldOn(index));
                    }
                    TextInputAction::Changed(text) => {
                        let value = if binding.quoted {
                            format!("{text:?}")
                        } else {
                            text.trim().to_string()
                        };
                        if !value.is_empty() {
                            edits.push(Edit::Apply(format!("{}: {}", binding.prop, value)));
                        }
                    }
                    TextInputAction::Returned(..) | TextInputAction::KeyFocusLost => {
                        edits.push(Edit::HoldOff);
                    }
                    TextInputAction::Escaped => {
                        if let Some((row, origin)) = self.text_edit_origin.clone() {
                            if row == index {
                                edits.push(Edit::Revert(index, origin));
                            }
                        }
                        edits.push(Edit::HoldOff);
                    }
                    _ => {}
                },
                RowKind::Color if !is_swatch => match widget_action.cast::<TextInputAction>() {
                    // The hex box: live-apply only when the text parses;
                    // Enter/blur accept, invalid input reverts.
                    TextInputAction::KeyFocus => {
                        edits.push(Edit::HoldOn(index));
                    }
                    TextInputAction::Changed(text) => {
                        if let Some((rgba, had_alpha)) = parse_hex(&text) {
                            let hex = format_hex(rgba, had_alpha || true);
                            edits.push(Edit::Apply(format!("{}: {}", binding.prop, hex)));
                        }
                    }
                    TextInputAction::Returned(text, _) => {
                        match parse_hex(&text) {
                            Some((rgba, _)) => {
                                let hex = format_hex(rgba, true);
                                edits.push(Edit::Apply(format!("{}: {}", binding.prop, hex)));
                            }
                            None => {
                                // Invalid: revert the field to the current value.
                                edits.push(Edit::Revert(index, binding.value.clone()));
                            }
                        }
                        edits.push(Edit::HoldOff);
                    }
                    TextInputAction::KeyFocusLost => {
                        edits.push(Edit::HoldOff);
                    }
                    TextInputAction::Escaped => {
                        edits.push(Edit::Revert(index, binding.value.clone()));
                        edits.push(Edit::HoldOff);
                    }
                    _ => {}
                },
                RowKind::Color => match widget_action.cast::<FabColorPickAction>() {
                    FabColorPickAction::Changed(v) => {
                        let hex = format_hex([v.x, v.y, v.z, v.w], true);
                        edits.push(Edit::Apply(format!("{}: {}", binding.prop, hex)));
                    }
                    FabColorPickAction::Opened => {
                        edits.push(Edit::HoldOn(index));
                    }
                    FabColorPickAction::Closed => {
                        edits.push(Edit::HoldOff);
                    }
                    _ => {}
                },
            }
        }
        for edit in edits {
            match edit {
                Edit::Apply(chunk) => self.sidebar_apply(cx, &sel, &chunk),
                Edit::Revert(index, value) => {
                    // Push the origin value back through the same path, then
                    // refresh the field itself.
                    if let Some(row) = self.rows.get(index) {
                        let text = if row.quoted {
                            format!("{value:?}")
                        } else {
                            value.clone()
                        };
                        self.sidebar_apply(cx, &sel, &format!("{}: {}", row.prop, text));
                    }
                    self.rows_uid = 0;
                }
                Edit::HoldOn(index) => {
                    session().lock().unwrap().edit_hold = true;
                    let origin = self.rows.get(index).map(|r| r.value.clone());
                    if let Some(origin) = origin {
                        self.text_edit_origin = Some((index, origin));
                    }
                    self.redraw_overlay(cx);
                }
                Edit::HoldOff => {
                    session().lock().unwrap().edit_hold = false;
                    self.text_edit_origin = None;
                    self.redraw_overlay(cx);
                }
            }
        }
    }

    /// Scroll the property list so the cascade level's header row is at
    /// the top (origin-dot click-through).
    fn scroll_to_cascade_level(&mut self, cx: &mut Cx, level: usize) {
        let entries = self.build_visible();
        let target = entries.iter().position(|entry| {
            matches!(entry, VisKind::Prop(index)
                if self.rows[*index].section == SectionKind::Cascade
                    && self.rows[*index].prop.starts_with(&format!("L{level} ")))
        });
        if let (Some(target), Some(sidebar)) = (target, self.sidebar.as_ref()) {
            let list_ref = sidebar.child(live_id!(props));
            {
                if let Some(mut list) = list_ref.borrow_mut::<PortalList>() {
                    list.set_first_id_and_scroll(target, 0.0);
                }
            }
            drop(list_ref);
        }
        let _ = cx;
    }

    fn redraw_sidebar(&mut self, cx: &mut Cx) {
        if let Some(sidebar) = &self.sidebar {
            sidebar.redraw(cx);
        }
        if let Some(sidebar_list) = &self.sidebar_list {
            sidebar_list.redraw(cx);
        }
        cx.redraw_all();
    }

    /// The four corner-handle centres for the pinned rect (TL, TR, BR, BL).
    fn radius_handle_centers(rect: Rect) -> [Vec2d; 4] {
        const HANDLE_INSET: f64 = 5.0;
        [
            dvec2(rect.pos.x + HANDLE_INSET, rect.pos.y + HANDLE_INSET),
            dvec2(
                rect.pos.x + rect.size.x - HANDLE_INSET,
                rect.pos.y + HANDLE_INSET,
            ),
            dvec2(
                rect.pos.x + rect.size.x - HANDLE_INSET,
                rect.pos.y + rect.size.y - HANDLE_INSET,
            ),
            dvec2(
                rect.pos.x + HANDLE_INSET,
                rect.pos.y + rect.size.y - HANDLE_INSET,
            ),
        ]
    }

    /// Diagonal-inward unit direction per corner: dragging inward increases
    /// the radius, outward decreases it.
    fn radius_inward(corner: usize) -> Vec2d {
        match corner {
            0 => dvec2(1.0, 1.0),
            1 => dvec2(-1.0, 1.0),
            2 => dvec2(-1.0, -1.0),
            _ => dvec2(1.0, -1.0),
        }
    }

    /// The cascade level palette: instance orange, then blue, purple,
    /// green, gray tail — the same colors the origin dots and the cascade
    /// rows share.
    fn level_color(level: usize) -> Vec4f {
        match level {
            0 => vec4(1.0, 0.62, 0.13, 1.0),
            1 => vec4(0.19, 0.78, 1.0, 1.0),
            2 => vec4(0.72, 0.5, 1.0, 1.0),
            3 => vec4(0.35, 0.85, 0.55, 1.0),
            _ => vec4(0.55, 0.55, 0.55, 1.0),
        }
    }

    /// Right edge of the app viewport: overlay drawing (outlines, tags,
    /// handles, strokes) never crosses into the panel band.
    fn overlay_max_x(&self, pass_size: Vec2d) -> f64 {
        if self.band.size.x > 0.0 {
            self.band.pos.x
        } else {
            pass_size.x
        }
    }

    /// Clip a rect to the app viewport; None when nothing remains visible.
    fn clip_to_viewport(&self, cx: &Cx2d, rect: Rect) -> Option<Rect> {
        let max_x = self.overlay_max_x(cx.current_pass_size());
        if rect.pos.x >= max_x {
            return None;
        }
        let mut out = rect;
        if out.pos.x + out.size.x > max_x {
            out.size.x = max_x - out.pos.x;
        }
        Some(out)
    }

    fn draw_pick(&mut self, cx: &mut Cx2d, pick: &TweakPick, style: PickStyle) {
        let dpi = cx.current_dpi_factor().max(1.0) as f32;
        self.draw_outline.dpi = dpi;
        match style {
            PickStyle::Pinned => {
                self.draw_outline.border_color = vec4(1.0, 0.62, 0.13, 1.0);
                self.draw_outline.fill_color = vec4(1.0, 0.62, 0.13, 0.08);
                self.draw_outline.border_size = 2.0;
                self.draw_outline.dash = 0.0;
            }
            PickStyle::Hover => {
                self.draw_outline.border_color = vec4(0.19, 0.78, 1.0, 1.0);
                self.draw_outline.fill_color = vec4(0.19, 0.78, 1.0, 0.06);
                self.draw_outline.border_size = 1.0;
                self.draw_outline.dash = 0.0;
            }
            PickStyle::PinnedQuiet => {
                // Tweaking in progress: a faint stippled HAIRLINE (one
                // device pixel), outset 1pt so the widget's own edge pixels
                // — the thing being judged — stay untouched.
                self.draw_outline.border_color = vec4(1.0, 0.62, 0.13, 0.25);
                self.draw_outline.fill_color = vec4(0.0, 0.0, 0.0, 0.0);
                self.draw_outline.border_size = 1.0 / dpi;
                self.draw_outline.dash = 1.0;
                let outset = Rect {
                    pos: dvec2(pick.rect.pos.x - 1.0, pick.rect.pos.y - 1.0),
                    size: dvec2(pick.rect.size.x + 2.0, pick.rect.size.y + 2.0),
                };
                if let Some(outset) = self.clip_to_viewport(cx, outset) {
                    self.draw_outline.draw_abs(cx, outset);
                }
                return; // no label chip, no fill
            }
        }
        let Some(outline_rect) = self.clip_to_viewport(cx, pick.rect) else {
            return; // fully under the panel: nothing to outline or label
        };
        self.draw_outline.draw_abs(cx, outline_rect);

        // The pick label: id path, type, rect (and spacing band when the
        // pointer is in the gap).
        let text = format!(
            "{} \u{2022} {} \u{2022} {:.0},{:.0} {:.0}\u{00d7}{:.0}{}",
            pick.path,
            pick.ty,
            pick.rect.pos.x,
            pick.rect.pos.y,
            pick.rect.size.x,
            pick.rect.size.y,
            match &pick.band {
                Some(band) => format!(" \u{2022} {band}"),
                None => String::new(),
            }
        );
        let pass_size = cx.current_pass_size();
        let max_x = self.overlay_max_x(pass_size);
        let label_height = 16.0;
        let approx_width = (text.chars().count() as f64) * 5.4 + 10.0;
        let mut pos = dvec2(pick.rect.pos.x, pick.rect.pos.y - label_height - 2.0);
        if pos.y < 0.0 {
            pos.y = (pick.rect.pos.y + pick.rect.size.y + 2.0).min(pass_size.y - label_height);
        }
        pos.x = pos.x.clamp(0.0, (max_x - approx_width).max(0.0));
        self.draw_label_bg.draw_abs(
            cx,
            Rect {
                pos,
                size: dvec2(approx_width, label_height),
            },
        );
        self.draw_label
            .draw_abs(cx, pos + dvec2(5.0, 2.0), &text);
    }

    /// Corner handles for the radius-like input (the first of the
    /// direct-manipulation handle family): fab-styled dots, visible whenever
    /// the pinned widget exposes a radius — and during the drag itself.
    fn draw_radius_handles(&mut self, cx: &mut Cx2d, rect: Rect) {
        let max_x = self.overlay_max_x(cx.current_pass_size());
        for center in Self::radius_handle_centers(rect) {
            if center.x + 4.0 > max_x {
                continue; // never draw handles into the panel band
            }
            self.draw_stroke.stroke_color = vec4(0.04, 0.04, 0.04, 0.9);
            self.draw_stroke.draw_abs(
                cx,
                Rect {
                    pos: dvec2(center.x - 4.0, center.y - 4.0),
                    size: dvec2(8.0, 8.0),
                },
            );
            self.draw_stroke.stroke_color = vec4(0.337, 0.502, 0.761, 1.0);
            self.draw_stroke.draw_abs(
                cx,
                Rect {
                    pos: dvec2(center.x - 3.0, center.y - 3.0),
                    size: dvec2(6.0, 6.0),
                },
            );
        }
    }

    fn draw_stroke_points(&mut self, cx: &mut Cx2d, points: &[(f64, f64)]) {
        let max_x = self.overlay_max_x(cx.current_pass_size());
        let points: Vec<(f64, f64)> = points
            .iter()
            .copied()
            .filter(|(x, _)| *x < max_x)
            .collect();
        let points = &points[..];
        // A freehand stroke as a dense dot chain: no polyline shader needed,
        // and grabs composite it exactly as seen.
        const RADIUS: f64 = 2.0;
        self.draw_stroke.stroke_color = vec4(1.0, 0.27, 0.27, 0.9);
        let mut last: Option<(f64, f64)> = None;
        for (x, y) in points.iter().copied() {
            if let Some((lx, ly)) = last {
                let dx = x - lx;
                let dy = y - ly;
                let dist = (dx * dx + dy * dy).sqrt();
                let steps = (dist / (RADIUS * 0.8)).ceil().max(1.0) as usize;
                for step in 1..=steps {
                    let t = step as f64 / steps as f64;
                    let px = lx + dx * t;
                    let py = ly + dy * t;
                    self.draw_stroke.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(px - RADIUS, py - RADIUS),
                            size: dvec2(RADIUS * 2.0, RADIUS * 2.0),
                        },
                    );
                }
            } else {
                self.draw_stroke.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(x - RADIUS, y - RADIUS),
                        size: dvec2(RADIUS * 2.0, RADIUS * 2.0),
                    },
                );
            }
            last = Some((x, y));
        }
    }
}

impl Widget for Tweaker {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !tweak_is_on() {
            return;
        }
        // The suppression window expired: bring the solid outline back.
        if self.next_frame.is_event(event).is_some() {
            let (until, hold) = {
                let s = session().lock().unwrap();
                (s.suppress_until, s.edit_hold)
            };
            let now = cx.seconds_since_app_start();
            if now < until || hold {
                self.next_frame = cx.new_next_frame();
            } else {
                self.redraw_overlay(cx);
            }
        }
        // Picking arrives pre-resolved through `window_intercept`; here the
        // tweaker handles its own chrome: the splitter, the sidebar, and
        // the fold / double-click-reset gestures on its rows.
        match event {
            Event::MouseDown(e) if Some(e.window_id.id()) == self.my_window => {
                let x = self.band.pos.x;
                if e.abs.x >= x - 3.0
                    && e.abs.x <= x + SPLITTER_WIDTH + 3.0
                    && e.abs.y >= self.band.pos.y
                {
                    self.splitter_drag = true;
                } else if e.abs.x > x
                    && !self
                        .open_popup
                        .is_some_and(|rect| rect.contains(e.abs))
                {
                    // Inside the sidebar band: section folds and the
                    // double-click-on-label reset gesture. Row areas are
                    // read here, at event time — they answer for the frame
                    // already on screen. A pointer inside an open popover
                    // belongs to the popover alone.
                    let hit = self
                        .visible
                        .iter()
                        .map(|row| (row.kind, row.item.area().clipped_rect(cx)))
                        .find(|(_, rect)| rect.size.y > 0.0 && rect.contains(e.abs));
                    if let Some((kind, rect)) = hit {
                        match kind {
                            VisKind::Section(section, _, _) => {
                                if self.filter.is_empty() {
                                    let index = section.index();
                                    self.collapsed[index] = !self.collapsed[index];
                                    self.redraw_sidebar(cx);
                                }
                            }
                            VisKind::More(section, _) => {
                                let index = section.index();
                                self.expanded[index] = !self.expanded[index];
                                self.redraw_sidebar(cx);
                            }
                            VisKind::Size
                            | VisKind::BoxInset(_)
                            | VisKind::FlowSpacing
                            | VisKind::AlignGrid => {}
                            VisKind::Prop(row_index) => {
                                // The origin-dot zone is the right edge:
                                // click jumps the cascade to that level.
                                let dot_zone = Rect {
                                    pos: dvec2(
                                        rect.pos.x + rect.size.x - 16.0,
                                        rect.pos.y,
                                    ),
                                    size: dvec2(16.0, rect.size.y),
                                };
                                let first = self.rows[row_index]
                                    .prop
                                    .split('.')
                                    .next()
                                    .unwrap_or("")
                                    .to_string();
                                if dot_zone.contains(e.abs)
                                    && self.rows[row_index].section != SectionKind::Cascade
                                {
                                    if let Some(&level) =
                                        self.origin_levels.get(&first)
                                    {
                                        self.collapsed
                                            [SectionKind::Cascade.index()] = false;
                                        self.scroll_to_cascade_level(cx, level);
                                        self.redraw_sidebar(cx);
                                        return;
                                    }
                                }
                                // The label zone is the row's left column.
                                let label_zone = Rect {
                                    pos: rect.pos,
                                    size: dvec2(
                                        104.0_f64.min(rect.size.x * 0.5),
                                        rect.size.y,
                                    ),
                                };
                                if label_zone.contains(e.abs) {
                                    let now = e.time;
                                    let double = self
                                        .last_label_click
                                        .is_some_and(|(t, row)| {
                                            row == row_index && now - t < 0.4
                                        });
                                    if double {
                                        self.last_label_click = None;
                                        let sel = session().lock().unwrap().pinned.clone();
                                        if let Some(sel) = sel {
                                            self.reset_row(cx, &sel, row_index);
                                        }
                                    } else {
                                        self.last_label_click = Some((now, row_index));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Event::KeyDown(ke)
                if ke.key_code == KeyCode::Slash
                    && tweak_is_on()
                    && cx.key_focus() == Area::Empty =>
            {
                // '/': jump to the filter (only when nothing is editing).
                self.focus_search_pending = true;
                self.redraw_sidebar(cx);
            }
            Event::MouseMove(e)
                if Some(e.window_id.id()) == self.my_window
                    && !self.splitter_drag
                    && tweak_is_on() =>
            {
                // The doc tooltip: a row whose prop carries doc-channel
                // text shows it, anchored to the row (no per-pixel churn).
                let mut new = None;
                if self.band.size.x > 0.0
                    && e.abs.x > self.band.pos.x
                    && !self
                        .open_popup
                        .is_some_and(|rect| rect.contains(e.abs))
                {
                    let hit = self
                        .visible
                        .iter()
                        .map(|row| (row.kind, row.item.area().clipped_rect(cx)))
                        .find(|(_, rect)| rect.size.y > 0.0 && rect.contains(e.abs));
                    if let Some((VisKind::Prop(index), rect)) = hit {
                        if let Some(doc) = self.row_docs.get(&self.rows[index].prop) {
                            let mut line = doc.lines().next().unwrap_or("").to_string();
                            if doc.lines().count() > 1 {
                                line.push_str(" \u{2026}");
                            }
                            new = Some(HoverDoc {
                                text: line,
                                pos: dvec2(rect.pos.x, rect.pos.y - 18.0),
                            });
                        }
                    }
                }
                if new != self.hover_doc {
                    self.hover_doc = new;
                    self.redraw_sidebar(cx);
                }
            }
            Event::MouseMove(e) if self.splitter_drag => {
                let window_right = self.band.pos.x + self.band.size.x;
                let width = (window_right - e.abs.x).clamp(180.0, 560.0);
                session().lock().unwrap().sidebar_width = width;
                cx.set_cursor(MouseCursor::ColResize);
                cx.redraw_all();
            }
            // ANY up releases the drag, wherever it lands — the capture
            // must never outlive the press.
            Event::MouseUp(_) => {
                self.splitter_drag = false;
            }
            // Safeties: focus loss or Escape frees the pointer too.
            Event::WindowLostFocus(_) => {
                self.splitter_drag = false;
            }
            Event::KeyDown(ke) if ke.key_code == KeyCode::Escape && self.splitter_drag => {
                self.splitter_drag = false;
            }
            _ => {}
        }
        // While a popover is open, wheel inside its rect must not scroll
        // the property list underneath it.
        let swallow_scroll = matches!(event, Event::Scroll(e)
            if self.open_popup.is_some_and(|rect| rect.contains(e.abs)));
        if let Some(sidebar) = self.sidebar.clone() {
            if !swallow_scroll {
                sidebar.handle_event(cx, event, scope);
            }
        }
        if let Event::Actions(actions) = event {
            self.handle_sidebar_actions(cx, actions);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        let on = tweak_is_on();
        if on && !self.was_on {
            // Opening the panel lands the caret in the filter.
            self.focus_search_pending = true;
        }
        self.was_on = on;
        let window_id = cx.get_current_window_id().map(|id| id.id());
        self.my_window = window_id;
        // Compress the app's UI while the sidebar is up; release it when the
        // mode goes off (this draw still runs once after the toggle because
        // set_tweak_on redraws everything).
        let desired = if on { sidebar_width() } else { 0.0 };
        self.ensure_body_margin(cx, desired);
        if !on {
            return DrawStep::done();
        }
        let (hover, pinned, strokes, live_stroke, suppress_until, edit_hold) = {
            let s = session().lock().unwrap();
            (
                s.hover.clone(),
                s.pinned.clone(),
                s.strokes.clone(),
                s.live_stroke.clone(),
                s.suppress_until,
                s.edit_hold,
            )
        };
        // While a value is actively moving (field focused / scrubbed, color
        // popover open, an apply within the last beat, a handle drag) the
        // pinned outline yields to a faint hairline stipple so the widget is
        // judged exactly as it renders.
        let now = cx.seconds_since_app_start();
        let quiet = edit_hold || now < suppress_until || self.radius_drag.is_some();
        if now < suppress_until {
            // Re-check when the linger expires.
            self.next_frame = cx.new_next_frame();
        }

        // Rebuild rows before the overlay: the radius handles need to know
        // whether the selection exposes a radius input.
        let sel = pinned.clone();
        let apply_gen = session().lock().unwrap().apply_gen;
        if let Some(sel_pick) = &sel {
            if self.rows_uid != sel_pick.uid || self.rows_gen != apply_gen {
                let path = sel_pick.path.clone();
                self.rebuild_rows(cx, sel_pick.uid, &path);
                self.rows_gen = apply_gen;
            }
        } else {
            self.rows.clear();
            self.rows_uid = 0;
            self.radius_prop = None;
        }

        let overlay_list = self.overlay_list.as_mut().unwrap();
        overlay_list.begin_overlay_reuse(cx);
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());

        if let Some(pick) = &pinned {
            if Some(pick.window_id) == window_id {
                let style = if quiet {
                    PickStyle::PinnedQuiet
                } else {
                    PickStyle::Pinned
                };
                self.draw_pick(cx, pick, style);
                // Direct-manipulation handles: the corner dots stay visible
                // even while quiet — they ARE the interaction.
                if self.radius_prop.is_some() {
                    self.draw_radius_handles(cx, pick.rect);
                }
            }
        }
        if !quiet {
            if let Some(pick) = &hover {
                let same = pinned.as_ref().is_some_and(|p| p.uid == pick.uid);
                if Some(pick.window_id) == window_id && !same {
                    self.draw_pick(cx, pick, PickStyle::Hover);
                }
            }
        }
        for stroke in &strokes {
            if Some(stroke.window_id) == window_id {
                self.draw_stroke_points(cx, &stroke.points);
            }
        }
        if let Some(stroke) = &live_stroke {
            if Some(stroke.window_id) == window_id {
                self.draw_stroke_points(cx, &stroke.points);
            }
        }

        cx.end_pass_sized_turtle();
        self.overlay_list.as_mut().unwrap().end(cx);

        // The property sidebar in its own overlay list, begun after the
        // outline overlay: the app (dock tab bars included) stacks below
        // it, and popups the panel opens (color pickers) begin later still,
        // so they stack above.
        let sidebar_list = self.sidebar_list.as_mut().unwrap();
        sidebar_list.begin_overlay_reuse(cx);
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());
        self.draw_sidebar(cx, scope, sel.as_ref());
        cx.end_pass_sized_turtle();
        self.sidebar_list.as_mut().unwrap().end(cx);

        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(seq: u64, path: &str, prop: &str, old: &str, new: &str) -> TweakDiffEntry {
        TweakDiffEntry {
            seq,
            path: path.to_string(),
            prop: prop.to_string(),
            old: old.to_string(),
            new: new.to_string(),
        }
    }

    #[test]
    fn diff_coalescing_keeps_first_old_and_last_new() {
        let entries = vec![
            entry(1, "a.b", "padding.left", "4", "10"),
            entry(2, "a.b", "padding.left", "10", "16"),
            entry(3, "a.c", "color", "#ff0000ff", "#00ff00ff"),
            entry(4, "a.b", "padding.left", "16", "24"),
        ];
        let coalesced = coalesce_diff(&entries);
        assert_eq!(coalesced.len(), 2);
        assert_eq!(coalesced[0].path, "a.b");
        assert_eq!(coalesced[0].old, "4");
        assert_eq!(coalesced[0].new, "24");
        assert_eq!(coalesced[1].path, "a.c");
    }

    #[test]
    fn diff_coalescing_drops_churn_back_to_original() {
        let entries = vec![
            entry(1, "a.b", "height", "40", "60"),
            entry(2, "a.b", "height", "60", "40"),
            entry(3, "a.b", "width", "10", "20"),
        ];
        let coalesced = coalesce_diff(&entries);
        assert_eq!(coalesced.len(), 1);
        assert_eq!(coalesced[0].prop, "width");
    }

    #[test]
    fn json_strings_escape() {
        assert_eq!(json_str("a\"b\n"), "\"a\\\"b\\n\"");
    }

    #[test]
    fn single_prop_chunks_parse() {
        assert_eq!(
            single_prop_chunk("padding.left: 12"),
            Some(("padding.left".to_string(), "12".to_string()))
        );
        assert_eq!(
            single_prop_chunk("{draw_bg.border_radius: 8}"),
            Some(("draw_bg.border_radius".to_string(), "8".to_string()))
        );
        // multi-property or nested chunks are not single props
        assert_eq!(single_prop_chunk("{a: 1, b: 2}"), None);
        assert_eq!(single_prop_chunk("padding: Inset{left: 4}"), None);
        assert_eq!(single_prop_chunk("draw_bg +: {color: #f00}"), None);
    }

    #[test]
    fn fmt_f64_stays_compact() {
        assert_eq!(fmt_f64(12.0), "12");
        assert_eq!(fmt_f64(2.5), "2.5");
        assert_eq!(fmt_f64(0.33333333), "0.3333");
    }
}
