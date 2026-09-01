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
    fab_controls::{format_hex, parse_hex, rgb_to_hsv, FabColorPick, FabColorPickAction, FabValueInput, FabValueInputAction},
    makepad_draw::makepad_platform::sploded::{SPLODED_SPREAD_DEFAULT, SPLODED_SPREAD_MAX, SPLODED_SPREAD_MIN},
    file_tree::{FileTree, FileTreeAction},
    label::Label,
    makepad_derive_widget::*,
    makepad_draw::*,
    portal_list::PortalList,
    text_input::TextInputAction,
    view::View,
    widget::*,
    widget_tree::{live_id_token, widget_type_names, CxWidgetExt},
};
use crate::makepad_script::script_eval;
use crate::Animate;
use crate::ButtonAction;
use crate::tooltip::Tooltip;
use crate::animator::{AnimatorState, Ease as AnimEase, Play};
use crate::makepad_draw::makepad_platform::DrawShaderId;
use crate::makepad_script::trap::NoTrap;
use crate::makepad_script::{parse_doc_hint, ScriptHeap, ScriptMod, ScriptObject};
use std::collections::{HashMap, HashSet};
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
#[derive(Clone, Debug, PartialEq, Default)]
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
    /// The plane the widget renders on in the exploded view (its component
    /// nesting depth); 0 when it has not drawn while the mode was up.
    pub level: usize,
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
    /// Where the widget's source object was constructed — `file:line` of
    /// the literal to edit. For a widget built from a template (a tab, a
    /// list item) this is the TEMPLATE's `:=` site, not the instance: that
    /// is what the AI rewrites so every instance follows.
    pub origin: String,
    /// How many other live widgets share that source object and received
    /// the same edit (0 for an ordinary, one-off widget).
    pub siblings: u32,
    /// "this" — specialise this instance (origin = its own site) — or
    /// "all" — every widget of the type (origin = the type's definition).
    pub scope: String,
}

/// A Ctrl+Space note card, attached to a widget by path: it rides with
/// the widget's live rect at (dx, dy) offset and is the human's text
/// channel to the AI (/tweak/state carries it).
#[derive(Clone, Debug)]
pub struct TweakNote {
    pub path: String,
    pub text: String,
    pub dx: f64,
    pub dy: f64,
}

/// One undoable edit gesture. Value: a contiguous run of applies to one
/// prop (a scrub down->up, a text commit, chevron steps) — old is the
/// pre-gesture value, new the latest. Reset: a double-click reset, with
/// the pruned ledger entries so undo can restore them.
#[derive(Clone, Debug)]
enum UndoStep {
    Value {
        path: String,
        prop: String,
        old: String,
        new: String,
        /// seq of the gesture's first ledger entry: undo removes every
        /// entry for (path, prop) from here on — as if never touched.
        seq_start: u64,
    },
    Reset {
        path: String,
        prop: String,
        removed: Vec<TweakDiffEntry>,
    },
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
    /// Live pointer position (window abs), for hover-revealed handles.
    pointer_abs: Vec2d,
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
    /// Eyedropper armed for this prop: the next press in the body samples
    /// the pixel under it instead of picking.
    eyedrop: Option<String>,
    /// Shader fns applied live from the source view: (uid, layer, fn name)
    /// → the text as applied, shown in place of the file's version.
    fn_overrides: HashMap<(u64, String, String), String>,
    /// What the panel says under the prompt: "sent … waiting", "applied",
    /// or an error — the send must be visible, and so must the landing.
    vibe_status: String,
    /// Bumped by every fn apply that did NOT come from the source view (the
    /// AI, /tweak/apply): the view re-reads its text for those only —
    /// otherwise a person's half-typed edit would be replaced under them.
    fn_override_gen: u64,
    /// A remote undo (true) / redo (false) request, consumed by the
    /// tweaker's event loop (Cmd+Z / Shift+Cmd+Z by the bridge).
    undo_redo: Option<bool>,
    /// A remote pulse request: a theme colour name (or #rrggbbaa) to pulse
    /// app-wide until an empty request clears it; consumed by the tweaker.
    pulse_req: Option<String>,
    /// The open colour popover's window rect, for /tweak/state.
    popup: Option<Rect>,
    /// A remote lock on the pulse mix (deterministic grabs): the pulse
    /// holds that tone instead of animating. None animates.
    pulse_lock: Option<f32>,
    /// The live theme pulse, shared by the tweaker's tick and the
    /// post-draw hook (taken out of the lock while either runs).
    pulse: Option<PulseState>,
    /// Theme colour edits in flight: name -> the original colour and the
    /// value every use of it now shows (re-applied after each draw).
    theme_overrides: Vec<(String, PulseState)>,
    /// A remote theme edit (`op=theme name= value=`), consumed by the tweaker.
    theme_req: Option<(String, String)>,
    /// Remote pose lock for the state swatches: Some(0..1) freezes every
    /// track at that mix (deterministic grabs), None animates.
    states_lock: Option<f64>,
    /// The pinned widget's animator groups, for /tweak/state.
    state_names: Vec<String>,
    /// Edit scope: false = "this" (specialise this instance), true = "all"
    /// (every live widget of the type; the ledger names the type's site).
    scope_all: bool,
    /// A prompt is out and unanswered (path, layer).
    vibe_pending: Option<(String, String)>,
    /// A sample in flight: (probe id, prop). Answered from the next frame.
    eyedrop_probe: Option<(u64, String)>,
    /// The press went to a navigation widget (tab, fold, dropdown) and was
    /// NOT consumed; its release must flow through too.
    pass_up: bool,
    /// The press went to the tweaker's own popover (colour picker) while it
    /// held the edit; the release must reach it too or its buttons never
    /// complete a click.
    hold_up: bool,
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
    /// Vibecode prompts sent this session: (sel path, layer, prompt).
    /// Surfaced in /tweak/state as the agent's work queue.
    vibes: Vec<(String, String, String, String)>,
    /// Ctrl+Space note cards, keyed by widget path (one per widget).
    notes: Vec<TweakNote>,
    /// The undo stack over edit gestures (Cmd+Z / Cmd+Shift+Z).
    undo: Vec<UndoStep>,
    redo: Vec<UndoStep>,
    /// True while the current gesture may still merge into the top undo
    /// step (closed by HoldOff so two scrubs never merge).
    undo_open: bool,
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
            drop(s);
            // F12 closes the whole design surface: the exploded view goes
            // with the panel (deferred toggle — performed pre-dispatch at
            // the next event), the marks and the flat band with it, so
            // the app is never left tilted without its panel.
            if cx.sploded_will_be_active() {
                cx.sploded_toggle();
            }
            cx.sploded_set_marks(None, None);
            cx.sploded_set_flat_band(None);
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

fn pick_candidate(
    cx: &Cx,
    widget: &WidgetRef,
    abs: Vec2d,
    attached: &HashSet<DrawListId>,
) -> Option<Rect> {
    if !widget.visible() {
        return None;
    }
    // Only what is on screen: a hidden Dock page keeps its retained draw
    // list and every rect in it, and clicking "through" to those was the
    // stale-tab bug.
    if !widget.area().is_attached(cx, attached) {
        return None;
    }
    // All instances, not the first: a text run's area is its glyph run,
    // and the first glyph alone is not a paragraph.
    let rect = widget.area().clipped_rect_union(cx);
    if rect.size.x <= 0.0 || rect.size.y <= 0.0 || !rect.contains(abs) {
        return None;
    }
    Some(rect)
}

/// The draw lists on screen, over every pass: a hidden page's list hangs
/// off no pass at all, so the union is exactly "visible". Computed at EVENT
/// time (between frames, when every list has linked into its parent) and
/// cached for the overlay draw, which runs mid-frame — while a list is
/// still open it has not linked into its parent yet, so a walk taken then
/// missed every page inside the dock (the "only tabs light up" bug).
fn attached_lists_of(cx: &Cx, _widget: &WidgetRef) -> HashSet<DrawListId> {
    let mut out = HashSet::new();
    for pass_id in cx.passes.id_iter() {
        out.extend(cx.attached_draw_lists(pass_id));
    }
    *attached_cache().lock().unwrap() = out.clone();
    out
}

fn attached_cache() -> &'static Mutex<HashSet<DrawListId>> {
    static CACHE: OnceLock<Mutex<HashSet<DrawListId>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashSet::new()))
}

/// A widget's on-screen rect this frame: empty when it is not drawn (its
/// page is hidden), whatever its retained draw list still holds.
/// A widget's on-screen rect this frame: empty when it is not drawn (its
/// page is hidden), whatever its retained draw list still holds. Draw-time
/// variant: the lists still open right now count as attached, because a
/// list links into its parent only when it ends.
/// A widget's on-screen rect this frame: empty when it is not drawn (its
/// page is hidden), whatever its retained draw list still holds. Uses the
/// attachment set the last EVENT computed (see `attached_lists_of`): the
/// overlay draws mid-frame, when open lists are not yet linked.
fn live_rect(cx: &Cx2d, widget: &WidgetRef) -> Rect {
    let attached = attached_cache().lock().unwrap().clone();
    if !attached.is_empty() && !widget.area().is_attached(cx, &attached) {
        return Rect::default();
    }
    // Attached = on screen; read the geometry even if a retained list left
    // the area one redraw stale (the tab strip does).
    widget.area().clipped_rect_union_attached(cx)
}


/// Navigation-class widgets keep working under the pick: "since tabs show
/// whole new chunks of clickable UI", a plain click on a tab, fold button,
/// dropdown opener or stack-navigation control both PINS it and performs
/// its normal action, so every corner of the app stays reachable while
/// tweaking. Fold headers and expandable panels count only for their
/// `header` subtree — a button in a fold's body is content, not navigation.
fn is_navigation_pick(cx: &mut Cx, uid: WidgetUid) -> bool {
    const NAV: &[&str] = &["Tab", "TabBar", "FoldButton", "DropDown", "StackNavigation"];
    const HEADER_NAV: &[&str] = &["FoldHeader", "ExpandablePanel"];
    let mut cur = Some(uid);
    let mut child_name: Option<LiveId> = None;
    for _ in 0..16 {
        let Some(u) = cur else { break };
        let widget = cx.widget_tree().widget(u);
        if widget.is_empty() {
            break;
        }
        let ty = widget
            .widget_type_id()
            .and_then(|type_id| widget_type_names(cx).get(&type_id).copied())
            .map(live_id_token)
            .unwrap_or_default();
        if NAV.contains(&ty.as_str()) {
            return true;
        }
        if HEADER_NAV.contains(&ty.as_str()) {
            return child_name == Some(live_id!(header));
        }
        child_name = cx.widget_tree().name_of(u);
        cur = cx.widget_tree().parent_of(u);
    }
    false
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
    max_level: Option<usize>,
    attached: &HashSet<DrawListId>,
    best: &mut Option<(WidgetRef, Rect, usize)>,
) {
    if !widget.visible() {
        return;
    }
    // Exploded view: the cursor is on ONE plane. A widget nested deeper than
    // that plane sits on another sheet, however its 2D rect overlaps the
    // un-projected point — that is what makes a covered parent selectable.
    let on_deeper_plane = max_level.is_some_and(|max| {
        cx.sploded_depth_of(widget.widget_uid().0)
            .is_some_and(|level| level > max)
    });
    if !on_deeper_plane && !is_design_transparent(widget) {
        if let Some(rect) = pick_candidate(cx, widget, abs, attached) {
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
        walk_pick(cx, &child, abs, depth + 1, max_level, attached, best);
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
        let child_rect = child.area().clipped_rect_union(cx);
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
        let r = child.area().clipped_rect_union(cx);
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
    // Exploded: the router already re-addressed `abs` onto the plane the ray
    // hit; a miss (off the deck) picks nothing.
    let max_level = if cx.sploded_active() {
        Some(cx.sploded_hit_level()?)
    } else {
        None
    };
    let attached = attached_lists_of(cx, root);
    // Start below the root (the window body itself is chrome, not content).
    root.children(&mut |_id, child| {
        walk_pick(cx, &child, abs, 0, max_level, &attached, &mut best);
    });
    let (widget, rect, _depth) = best?;
    let uid = widget.widget_uid();
    let level = cx.sploded_depth_of(uid.0).unwrap_or(0);
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
        level,
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
    // F12 toggles the mode, bridge or no bridge: the design surface is
    // in-process and owes the remote nothing. Only the HTTP endpoints and
    // the AI vibecode loop need --remote; without it they simply are not
    // there, and the panel still is.
    //
    // SHIFT+F12 is not ours: that is the screen recorder
    // (widgets/src/screen_cap.rs), and it must not drag the design surface
    // into every recording.
    if let Event::KeyDown(key_event) = event {
        if key_event.key_code == KeyCode::F12 && !key_event.modifiers.shift {
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
    {
        // Hover-reveal for the radius handles: redraw the overlay when the
        // pointer crosses into (or out of) reach of a pinned corner, so the
        // dots appear and vanish without waiting for another repaint cause.
        let mut sess = session().lock().unwrap();
        let was = sess.pointer_abs;
        sess.pointer_abs = abs;
        if let Some(pin) = sess.pinned.clone() {
            let near_of = |p: Vec2d| {
                Tweaker::radius_handle_centers(pin.rect).iter().any(|c| {
                    let dx = p.x - c.x;
                    let dy = p.y - c.y;
                    dx * dx + dy * dy <= 28.0 * 28.0
                })
            };
            if near_of(was) != near_of(abs) {
                drop(sess);
                if let Some(mut tw) = tweaker.borrow_mut::<Tweaker>() {
                    tw.redraw_overlay(cx);
                }
            }
        }
    }

    // Scroll routes by REGION, exclusively. Over the panel band: the
    // tweaker tree alone gets it (the body extends under the panel and
    // double-scrolled otherwise). Over the body: ordinary dispatch — the
    // app scrolls, and the overlay re-reads live rects each frame so the
    // outlines follow the content.
    if kind == PointerKind::Scroll {
        let in_band = tweaker
            .borrow::<Tweaker>()
            .map(|tw| tw.band.size.x > 0.0 && tw.band.contains(abs))
            .unwrap_or(false);
        if in_band {
            tweaker.handle_event(cx, event, &mut Scope::empty());
            return true;
        }
        return false;
    }

    // The note card lives on the CANVAS but belongs to the tweaker: input
    // inside it goes to ordinary dispatch (never picked through).
    {
        let note_hit = tweaker
            .borrow::<Tweaker>()
            .and_then(|tw| tw.note_rect)
            .map(|rect| {
                if let Event::MouseDown(e) = event {
                    rect.contains(e.abs)
                } else if let Event::MouseMove(e) = event {
                    rect.contains(e.abs)
                } else if let Event::MouseUp(e) = event {
                    rect.contains(e.abs)
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if note_hit {
            if kind == PointerKind::Down {
                log!("TWEAK press {:.0},{:.0} on the note card: not a pick", abs.x, abs.y);
            }
            return false;
        }
    }
    // A scrub pin owns the pointer ABSOLUTELY: no picking, no hover-outline
    // churn at the virtual abs (the "keeps highlighting things while I
    // drag" bug), and never a consumed up (the wedge). The pick pass
    // stands down until the pin releases; stale hover chrome clears once.
    // A NEW press while a pin stands means the pin's release was lost (the
    // button cannot be held twice): drop it and pick, instead of feeding
    // one whole click to a gesture that ended.
    if cx.fingers.has_pinned_capture() {
        if kind == PointerKind::Down {
            log!("TWEAK press {:.0},{:.0} with a stale scrub pin: released, picking", abs.x, abs.y);
            cx.unpin_pointer_capture();
        } else {
            let had_hover = {
                let mut s = session().lock().unwrap();
                s.hover.take().is_some()
            };
            if had_hover {
                cx.redraw_all();
            }
            return false;
        }
    }
    // A splitter drag owns the pointer outright: the pick path must not
    // eat the Move that resizes or the Up that releases — releasing
    // OUTSIDE the band swallowed the Up and wedged the drag forever. The
    // same stale-gesture rule as the pin: a new press ends it.
    {
        let dragging = tweaker
            .borrow::<Tweaker>()
            .map(|tw| tw.splitter_drag)
            .unwrap_or(false);
        if dragging {
            if kind == PointerKind::Down {
                log!("TWEAK press {:.0},{:.0} with a stale splitter drag: ended, picking", abs.x, abs.y);
                if let Some(mut tw) = tweaker.borrow_mut::<Tweaker>() {
                    tw.splitter_drag = false;
                }
            } else {
                if kind == PointerKind::Move {
                    // Keep the resize cursor for the whole drag, wherever
                    // the pointer wanders.
                    cx.set_cursor(MouseCursor::EwResize);
                }
                return false;
            }
        }
    }
    // A mouse-up always pairs with the down that started it: if we swallowed
    // the down, swallow the up wherever it lands.
    let finish_consumed_down = kind == PointerKind::Up && session().lock().unwrap().down_consumed;
    if !body_rect.contains(abs) && !finish_consumed_down {
        // Outside the body (caption bar, sidebar band): ordinary dispatch.
        if kind == PointerKind::Down {
            log!(
                "TWEAK press {:.0},{:.0} outside the body {:.0},{:.0} {:.0}x{:.0}: not a pick",
                abs.x, abs.y, body_rect.pos.x, body_rect.pos.y, body_rect.size.x, body_rect.size.y
            );
        }
        return false;
    }
    // The tweaker's own UI is never a pick target. A body that was not
    // vacated (apps whose layout ignores the sidebar apply) still overlaps
    // the band, so check it explicitly: a pointer over the panel goes to
    // ordinary dispatch — the panel wins INPUT, and the app widget behind
    // it can never be selected through it.
    if !finish_consumed_down {
        let band = tweaker.borrow::<Tweaker>().map(|tw| tw.band).unwrap_or_default();
        // The panel splitter announces itself: the resize cursor over its
        // grab band (it is the intercept's own gesture, not a Splitter
        // widget, so nothing else would set one).
        if kind == PointerKind::Move
            && band.size.x > 0.0
            && abs.x >= band.pos.x - 3.0
            && abs.x <= band.pos.x + SPLITTER_WIDTH + 3.0
            && abs.y >= band.pos.y
        {
            cx.set_cursor(MouseCursor::EwResize);
            return false;
        }
        let in_band = band.size.x > 0.0 && band.contains(abs);
        if in_band {
            if kind == PointerKind::Down {
                log!("TWEAK press {:.0},{:.0} in the panel band: not a pick", abs.x, abs.y);
            }
            if kind == PointerKind::Move {
                // The body's pick-hand must not linger over the panel; the
                // panel's own widgets set theirs (I-beam etc.) after this.
                cx.set_cursor(MouseCursor::Default);
            }
            return false;
        }
    }

    // Eyedropper armed: the press samples the pixel under it from the next
    // presented frame (device pixels), the tweaker's event loop applies it.
    if kind == PointerKind::Down {
        let armed = session().lock().unwrap().eyedrop.take();
        if let Some(prop) = armed {
            let dpi = cx.windows[window_id].window_geom.dpi_factor;
            let id = cx.probe_pixel((abs.x * dpi) as u32, (abs.y * dpi) as u32);
            session().lock().unwrap().eyedrop_probe = Some((id, prop));
            session().lock().unwrap().down_consumed = true;
            return true;
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
            // The colour popover overhangs the body: a move inside it is
            // the panel's (its palette strip hovers), not a hover-pick.
            let popup = session().lock().unwrap().popup;
            if popup.is_some_and(|rect| rect.contains(abs)) {
                tweaker.handle_event(cx, event, &mut Scope::empty());
                return true;
            }
            // The doc tooltip closes when the pointer leaves into the body
            // (the panel never sees these moves).
            if let Some(mut tw) = tweaker.borrow_mut::<Tweaker>() {
                tw.doc_tip_hover(cx, abs);
                tw.states_hover(cx, abs);
                tw.pulse_hover(cx, abs);
            }
            let eyedrop = session().lock().unwrap().eyedrop.is_some();
            cx.set_cursor(if annotate || eyedrop {
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
                // The matching release goes the same way, so a button
                // inside the popover (the eyedropper's `pick`) can finish
                // its click.
                let mut s = session().lock().unwrap();
                s.edit_hold = false;
                s.hold_up = true;
                drop(s);
                redraw_tweaker(cx, &tweaker);
                // A press on the tweaker's own chrome over the body (the
                // colour popover) stops here. A press on the APP both ends
                // the field/popover and picks what it landed on — one
                // click, like a click with nothing focused; the first click
                // after typing in a field used to be swallowed.
                let attached = attached_lists_of(cx, &tweaker);
                if chrome_hit(cx, &tweaker, abs, &attached, 0) {
                    return true;
                }
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
                // Navigation stays reachable: the press flows on to the
                // tab / fold / dropdown as well, and so will its release.
                if let Some(pick) = &pick {
                    if is_navigation_pick(cx, WidgetUid(pick.uid)) {
                        let mut s = session().lock().unwrap();
                        s.down_consumed = false;
                        s.pass_up = true;
                        return false;
                    }
                }
            }
        }
        PointerKind::Up => {
            let mut s = session().lock().unwrap();
            if s.pass_up {
                s.pass_up = false;
                s.down_consumed = false;
                return false;
            }
            if s.hold_up {
                s.hold_up = false;
                s.down_consumed = false;
                drop(s);
                tweaker.handle_event(cx, event, &mut Scope::empty());
                redraw_tweaker(cx, &tweaker);
                return true;
            }
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

/// Does a point land on the tweaker's own widgets (below its root overlay,
/// which spans the window)?
fn chrome_hit(
    cx: &Cx,
    widget: &WidgetRef,
    abs: Vec2d,
    attached: &HashSet<DrawListId>,
    depth: usize,
) -> bool {
    if depth > 0 && pick_candidate(cx, widget, abs, attached).is_some() {
        return true;
    }
    if depth > 24 {
        return false;
    }
    let mut kids: Vec<WidgetRef> = Vec::new();
    widget.children(&mut |_id, child| kids.push(child));
    kids
        .iter()
        .any(|child| chrome_hit(cx, child, abs, attached, depth + 1))
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


/// Apply `sel`'s draw-layer SOURCE object + the session-ledger overlay onto
/// a preview quad — the swatch then renders with the selection's actual
/// compiled shader (fn-hash cache) at its live-tweaked values. Quad-family
/// layers only; anything else leaves the preview untouched.
/// Re-indent a fn's source for the view: drop the common leading indent and
/// turn every remaining 4-space (or tab) step into 2 spaces — "much less
/// aggressive indenting like 2 spaces per tab".
fn reindent_two(src: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let common = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { 4 } else { 1 }).sum::<usize>())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| {
            let indent: usize = l.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { 4 } else { 1 }).sum();
            let rest = l.trim_start_matches([' ', '\t']);
            let level = indent.saturating_sub(common) / 4;
            format!("{}{}", "  ".repeat(level), rest)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Remember `name: fn() {…}` entries of an applied `<layer> +: { … }` chunk
/// so the source view shows what runs, not what the file says.
fn record_fn_overrides(uid: u64, chunk: &str) {
    let chunk = chunk.trim();
    let Some(open) = chunk.find("+:") else { return };
    let layer = chunk[..open].trim().to_string();
    let Some(body) = chunk[open..].find('{').map(|i| &chunk[open + i + 1..]) else { return };
    let body = body.trim_end().trim_end_matches('}');
    let mut s = session().lock().unwrap();
    s.fn_override_gen += 1;
    for name in ["pixel", "vertex"] {
        if let Some(seg) = body.split(&format!("{name}:")).nth(1) {
            let seg = format!("{name}:{seg}");
            s.fn_overrides.insert((uid, layer.clone(), name.to_string()), reindent_two(seg.trim_end()));
        }
    }
}

/// Ctrl+Enter in the source view: apply the fn as edited to the pinned
/// widget's layer, live — no AI needed for a hand edit.
fn apply_fn_edit(cx: &mut Cx, tweaker: &mut Tweaker, text: &str) -> Result<(), String> {
    let Some(sel) = session().lock().unwrap().pinned.clone() else {
        return Err("nothing pinned".to_string());
    };
    let layer = tweaker.vibe_layer.clone().unwrap_or_else(|| "draw_bg".to_string());
    // Strip the `// name — loc` header lines the view adds; what is left is
    // one or more `name: fn() { … }` entries — a valid layer body.
    let body: String = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("// "))
        .collect::<Vec<_>>()
        .join("\n");
    let chunk = format!("{layer} +: {{\n{body}\n}}");
    match apply_splash_chunk(cx, &cx.widget_tree().widget(WidgetUid(sel.uid)), &sel.path, &chunk, "editor") {
        Ok(_) => {
            log!("TWEAK editor applied {} fn edit to {}", layer, sel.path);
            let mut s = session().lock().unwrap();
            for (name, _, _) in &tweaker.vibe_fn_sources {
                // Remember the applied text per fn so the view shows it.
                if let Some(seg) = body.split(&format!("{name}:")).nth(1) {
                    let seg = format!("{name}:{seg}");
                    s.fn_overrides.insert((sel.uid, layer.clone(), name.clone()), seg.trim_end().to_string());
                }
            }
            Ok(())
        }
        Err(error) => {
            log!("TWEAK editor apply failed: {error}");
            Err(error)
        }
    }
}

/// The script-defined shader fns of a widget's draw layer — `pixel` and
/// `vertex` when the layer (or a proto in its chain) sets one — as
/// (name, "file:line", source text). What the Shader tab shows under the
/// well and what a code-only prompt rewrites.
fn layer_fn_sources(cx: &mut Cx, widget: &WidgetRef, layer: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let source = widget.script_source();
    if source == ScriptObject::ZERO {
        return out;
    }
    let layer_id = LiveId::from_str(layer);
    cx.with_vm(|vm| {
        let value = vm.bx.heap.value(source, layer_id.into(), NoTrap);
        if value.as_object().is_none() {
            return;
        }
        // Closest level first: the fn the widget actually runs is the
        // nearest definition up the layer's construction chain (a `+:`
        // merge in the widget's script, else the draw type's own).
        let chain = vm.construction_chain(value);
        for (name, key) in [("pixel", id!(pixel)), ("vertex", id!(vertex))] {
            for lvl in &chain {
                if !lvl.own_keys.contains(&key) {
                    continue;
                }
                let f = vm.bx.heap.value(lvl.object, key.into(), NoTrap);
                let Some(fobj) = f.as_object() else { break };
                let Some(makepad_script::ScriptFnPtr::Script(ip)) = vm.bx.heap.as_fn(fobj) else {
                    break;
                };
                if let Some((loc, text)) = vm.bx.code.fn_source_text(ip) {
                    let base = loc.file.rsplit('/').next().unwrap_or(&loc.file).to_string();
                    out.push((name.to_string(), format!("{base}:{}", loc.line), text));
                }
                break;
            }
        }
    });
    out
}

/// One animator track (hover, down, focus, disabled…) of a widget, read
/// from its script cascade, ready to POSE the layer's instance slice: the
/// values the off and on states apply to this layer, per shader input.
#[derive(Clone)]
pub struct StateTrack {
    pub group: String,
    fields: Vec<StateField>,
    /// Into `on` from `off` (the track's own `from` map, `all` fallback).
    on_play: Play,
    on_ease: AnimEase,
    /// Back into `off`.
    off_play: Play,
    off_ease: AnimEase,
}

#[derive(Clone)]
struct StateField {
    id: LiveId,
    /// Slot values of the pose; None = whatever the live widget has.
    off: Option<Vec<f32>>,
    on: Option<Vec<f32>>,
}

impl StateTrack {
    /// Write the pose between off (0) and on (1) over the mirrored
    /// instance, by the shader's own instance layout.
    fn pose(&self, cx: &Cx, mirror: &mut MaterialMirror, mix: f32) {
        let Some(shader_id) = mirror.draw_vars.draw_shader_id else { return };
        let sh = &cx.draw_shaders[shader_id.index];
        for field in &self.fields {
            let Some(input) = sh.mapping.instances.inputs.iter().find(|i| i.id == field.id) else {
                continue;
            };
            for slot in 0..input.slots {
                let at = input.offset + slot;
                if at >= mirror.instance.len() {
                    break;
                }
                let live = mirror.instance[at];
                let a = field.off.as_ref().and_then(|v| v.get(slot).copied()).unwrap_or(live);
                let b = field.on.as_ref().and_then(|v| v.get(slot).copied()).unwrap_or(live);
                mirror.instance[at] = a + (b - a) * mix;
            }
        }
    }
}

/// Where a track is in its off→on→off cycle at time `t` (seconds since
/// the cycle began): the real transition (duration + ease from the
/// animator, a Snap jumps) each way, each pose held for a beat.
fn track_mix(track: &StateTrack, t: f64) -> f32 {
    const HOLD: f64 = 0.7;
    const MIN: f64 = 0.15;
    let on_d = play_duration(track.on_play).max(MIN);
    let off_d = play_duration(track.off_play).max(MIN);
    let cycle = on_d + HOLD + off_d + HOLD;
    let p = t.rem_euclid(cycle);
    if p < on_d {
        if matches!(track.on_play, Play::Snap) { 1.0 } else { track.on_ease.map(p / on_d) as f32 }
    } else if p < on_d + HOLD {
        1.0
    } else if p < on_d + HOLD + off_d {
        if matches!(track.off_play, Play::Snap) {
            0.0
        } else {
            1.0 - track.off_ease.map((p - on_d - HOLD) / off_d) as f32
        }
    } else {
        0.0
    }
}

fn play_duration(play: Play) -> f64 {
    match play {
        Play::Snap => 0.0,
        Play::Forward { duration }
        | Play::Reverse { duration, .. }
        | Play::Loop { duration, .. }
        | Play::ReverseLoop { duration, .. }
        | Play::BounceLoop { duration, .. } => duration,
    }
}

/// A state's `apply` values for one layer, as shader slots: numbers are
/// one slot, colours four (rgba 0..1), bools one.
fn pose_values(vm: &mut ScriptVm, state: &AnimatorState, layer_id: LiveId) -> Vec<(LiveId, Vec<f32>)> {
    let mut out = Vec::new();
    let Some(apply) = state.apply else { return out };
    let layer_value = vm.bx.heap.value(apply, layer_id.into(), NoTrap);
    let Some(layer_obj) = layer_value.as_object() else { return out };
    let mut keys: Vec<LiveId> = Vec::new();
    for lvl in vm.construction_chain(layer_value) {
        for key in &lvl.own_keys {
            if !keys.contains(key) {
                keys.push(*key);
            }
        }
    }
    for key in keys {
        let mut value = vm.bx.heap.value(layer_obj, key.into(), NoTrap);
        // `snap(1.0)` / `instance(x)` wrap the value in an object keyed
        // `value`; the pose is the wrapped scalar.
        if value.as_color().is_none() && value.as_bool().is_none() && value.as_number().is_none() {
            if let Some(obj) = value.as_object() {
                let inner = vm.bx.heap.value(obj, id!(value).into(), NoTrap);
                if !inner.is_nil() {
                    value = inner;
                }
            }
        }
        let slots = if let Some(c) = value.as_color() {
            vec![
                ((c >> 24) & 0xff) as f32 / 255.0,
                ((c >> 16) & 0xff) as f32 / 255.0,
                ((c >> 8) & 0xff) as f32 / 255.0,
                (c & 0xff) as f32 / 255.0,
            ]
        } else if let Some(b) = value.as_bool() {
            vec![if b { 1.0 } else { 0.0 }]
        } else if let Some(n) = value.as_number() {
            vec![n as f32]
        } else {
            continue;
        };
        out.push((key, slots));
    }
    out
}

/// The widget's animator tracks that touch `layer`, from its script
/// cascade (the instance's own animator, else its type's): per group the
/// default state is "off" and the first other state is "on".
fn animator_tracks(cx: &mut Cx, widget: &WidgetRef, layer: &str) -> Vec<StateTrack> {
    let mut out = Vec::new();
    let source = widget.script_source();
    if source == ScriptObject::ZERO {
        return out;
    }
    let layer_id = LiveId::from_str(layer);
    cx.with_vm(|vm| {
        let anim = vm.bx.heap.value(source, id!(animator).into(), NoTrap);
        let Some(anim_obj) = anim.as_object() else { return };
        let mut groups: Vec<LiveId> = Vec::new();
        for lvl in vm.construction_chain(anim) {
            for key in &lvl.own_keys {
                if !groups.contains(key) {
                    groups.push(*key);
                }
            }
        }
        for group in groups {
            let group_value = vm.bx.heap.value(anim_obj, group.into(), NoTrap);
            let Some(group_obj) = group_value.as_object() else { continue };
            let mut default = LiveId(0);
            let mut states: Vec<LiveId> = Vec::new();
            for lvl in vm.construction_chain(group_value) {
                for key in &lvl.own_keys {
                    if *key == id!(default) {
                        continue;
                    }
                    if !states.contains(key) {
                        states.push(*key);
                    }
                }
            }
            if let Some(id) = vm.bx.heap.value(group_obj, id!(default).into(), NoTrap).as_id() {
                default = id;
            }
            if states.is_empty() {
                continue;
            }
            let off = if states.contains(&default) { default } else { states[0] };
            let Some(on) = states.iter().copied().find(|s| *s != off) else { continue };
            let off_state = AnimatorState::script_from_value(vm, vm.bx.heap.value(group_obj, off.into(), NoTrap));
            let on_state = AnimatorState::script_from_value(vm, vm.bx.heap.value(group_obj, on.into(), NoTrap));
            let off_values = pose_values(vm, &off_state, layer_id);
            let on_values = pose_values(vm, &on_state, layer_id);
            if off_values.is_empty() && on_values.is_empty() {
                // This track never touches the layer (a text-only state).
                continue;
            }
            let mut fields: Vec<StateField> = Vec::new();
            for (id, v) in &off_values {
                fields.push(StateField { id: *id, off: Some(v.clone()), on: None });
            }
            for (id, v) in &on_values {
                match fields.iter_mut().find(|f| f.id == *id) {
                    Some(f) => f.on = Some(v.clone()),
                    None => fields.push(StateField { id: *id, off: None, on: Some(v.clone()) }),
                }
            }
            let on_play = on_state
                .from
                .get(&off)
                .or_else(|| on_state.from.get(&id!(all)))
                .copied()
                .unwrap_or(Play::Forward { duration: 0.3 });
            let off_play = off_state
                .from
                .get(&on)
                .or_else(|| off_state.from.get(&id!(all)))
                .copied()
                .unwrap_or(Play::Forward { duration: 0.3 });
            out.push(StateTrack {
                group: format!("{}", live_id_token(group)),
                fields,
                on_play,
                on_ease: on_state.ease.unwrap_or(AnimEase::Linear),
                off_play,
                off_ease: off_state.ease.unwrap_or(AnimEase::Linear),
            });
        }
    });
    out
}

/// One live draw call's inputs, copied from a widget's area: what the
/// material swatch draws so it shows exactly what the widget shows.
struct MaterialMirror {
    /// The swatch's own DrawVars re-pointed at the widget's shader, with
    /// the widget's draw call uniforms, textures and geometry copied in.
    draw_vars: DrawVars,
    instance: Vec<f32>,
    rect_pos: Option<usize>,
    rect_size: Option<usize>,
    draw_clip: Option<usize>,
}

impl MaterialMirror {
    /// The widget's own rect size, from the copied instance.
    fn native_size(&self) -> Vec2d {
        match self.rect_size {
            Some(rs) => dvec2(self.instance[rs] as f64, self.instance[rs + 1] as f64),
            None => dvec2(64.0, 24.0),
        }
    }

    fn draw(&self, cx: &mut Cx2d, rect: Rect, clip: Option<(Vec2d, Vec2d)>) {
        let draw_vars = &self.draw_vars;
        let Some(mut many) = cx.begin_many_instances(draw_vars) else {
            return;
        };
        let mut inst = self.instance.clone();
        if let Some(rp) = self.rect_pos {
            inst[rp] = rect.pos.x as f32;
            inst[rp + 1] = rect.pos.y as f32;
        }
        if let Some(rs) = self.rect_size {
            inst[rs] = rect.size.x as f32;
            inst[rs + 1] = rect.size.y as f32;
        }
        if let Some(dc) = self.draw_clip {
            // The well is its own draw list with a magnifier transform, so
            // nothing upstream clips it: the caller passes the scroll
            // viewport's clip mapped into this (pre-transform) space, and a
            // well scrolling under the panel header is cut off exactly like
            // a text row.
            let (min, max) = clip.unwrap_or((rect.pos, rect.pos + rect.size));
            inst[dc] = min.x.max(rect.pos.x) as f32;
            inst[dc + 1] = min.y.max(rect.pos.y) as f32;
            inst[dc + 2] = max.x.min(rect.pos.x + rect.size.x) as f32;
            inst[dc + 3] = max.y.min(rect.pos.y + rect.size.y) as f32;
        }
        many.instances.extend_from_slice(&inst);
        let area = cx.end_many_instances(many);
        if std::env::var_os("MAKEPAD_TWEAK_TRACE").is_some() {
            if let Area::Instance(ia) = area {
                if let Some(dc) = cx.draw_lists[ia.draw_list_id].draw_items[ia.draw_item_id].draw_call() {
                    log!("TWEAK trace swatch copy shader={:?} inst={:?} uniforms[..24]={:?}", dc.draw_shader_id, &inst, &dc.dyn_uniforms[..24]);
                }
            }
        }
    }
}

/// Read a widget's live draw call through its area: the first instance's
/// values plus the call's shader, geometry, uniforms and textures.
fn capture_material_mirror(cx: &Cx, widget: &WidgetRef, area: Area, base: &DrawVars) -> Option<MaterialMirror> {
    let Area::Instance(inst) = area else {
        return None;
    };
    if inst.instance_count == 0 {
        return None;
    }
    let draw_list = &cx.draw_lists[inst.draw_list_id];
    let draw_item = &draw_list.draw_items[inst.draw_item_id];
    let draw_call = draw_item.draw_call()?;
    let buf = draw_item.instances.as_ref()?;
    let sh = &cx.draw_shaders[draw_call.draw_shader_id.index];
    let stride = sh.mapping.instances.total_slots;
    if stride == 0 || inst.instance_offset + stride > buf.len() {
        return None;
    }
    if std::env::var_os("MAKEPAD_TWEAK_TRACE").is_some() {
        log!(
            "TWEAK trace swatch source uid={} shader={:?} stride={} inst={:?} uniforms[..24]={:?}",
            widget.widget_uid().0,
            draw_call.draw_shader_id,
            stride,
            &buf[inst.instance_offset..inst.instance_offset + stride],
            &draw_call.dyn_uniforms[..24]
        );
    }
    let mut draw_vars = base.clone();
    draw_vars.area = Area::Empty;
    draw_vars.draw_shader_id = Some(draw_call.draw_shader_id);
    draw_vars.geometry_id = draw_call.geometry_id;
    draw_vars.options = draw_call.options.clone();
    draw_vars.dyn_uniforms = draw_call.dyn_uniforms;
    draw_vars.texture_slots = draw_call.texture_slots.clone();
    draw_vars.uniform_buffer_slots = draw_call.uniform_buffer_slots.clone();
    Some(MaterialMirror {
        draw_vars,
        instance: buf[inst.instance_offset..inst.instance_offset + stride].to_vec(),
        rect_pos: sh.mapping.rect_pos,
        rect_size: sh.mapping.rect_size,
        draw_clip: sh.mapping.draw_clip,
    })
}


/// Feed the undo stack from one applied ledger entry. Consecutive applies
/// to the same prop merge while the gesture is open (a scrub = one step);
/// any new user gesture clears the redo branch. Undo/redo replays pass
/// origin "undo"/"redo" and are not tracked.
fn track_undo(s: &mut TweakSession, entry: &TweakDiffEntry) {
    s.redo.clear();
    if s.undo_open {
        if let Some(UndoStep::Value { path, prop, new, .. }) = s.undo.last_mut() {
            if *path == entry.path && *prop == entry.prop {
                *new = entry.new.clone();
                return;
            }
        }
    }
    s.undo.push(UndoStep::Value {
        path: entry.path.clone(),
        prop: entry.prop.clone(),
        old: entry.old.clone(),
        new: entry.new.clone(),
        seq_start: entry.seq,
    });
    s.undo_open = true;
}

/// One level of the selection's construction chain, display-ready.
/// Built from `vm.construction_chain` over the widget's `#[source]` object:
/// the proto chain of `made_at` ips, each resolved to a source location and
/// its `///` docs (see platform/script/src/docs.rs).
#[derive(Clone)]
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
/// A reflected value that is a structured type gets a typed editor rather
/// than a `{..}`/`vec2f(..)` text dump. Determined from the dump text —
/// no reflection change — so it stays a pure display-layer concern.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StructKind {
    None,
    Vec2,
    Vec3,
    Vec4,
    Inset,
    Metrics,
    SizeField,
    /// Recognized as structured but with no editor yet (a big nested
    /// struct like a full text_style): shown collapsed, never dumped.
    NoEditor,
}

/// The hover pulse: every drawn value equal to a theme colour — dynamic
/// uniforms and colour instances across every draw call — is scaled in
/// brightness in place (CPU-side buffers, dirty flags set, uploaded next
/// frame). No ledger, no apply: stopping just redraws all, which rebuilds
/// every buffer from the widgets — restore is the ordinary draw path.
/// The pulsed form of a colour at mix amount `m` (0 = the true colour).
/// A brightness multiply is invisible on black or near-transparent theme
/// colours, so the pulse mixes rgb toward the contrast tone (white for
/// dark colours, black for light ones) and lifts alpha toward opaque —
/// visible for any colour, and exactly invertible via the same function.
fn pulse_tone(target: [f32; 4], m: f32) -> [f32; 4] {
    let lum = 0.299 * target[0] + 0.587 * target[1] + 0.114 * target[2];
    let tone = if lum < 0.5 { 1.0 } else { 0.0 };
    [
        target[0] + (tone - target[0]) * m,
        target[1] + (tone - target[1]) * m,
        target[2] + (tone - target[2]) * m,
        target[3] + (1.0 - target[3]) * m * 0.85,
    ]
}

/// One colour slot the pulse has written. Theme colours reach the GPU
/// four ways: per-instance values, a draw call's dyn uniforms, a shader
/// referencing the theme in its code (a per-shader scope uniform), and a
/// pass clear colour (the window background).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum PulseSlot {
    Inst { list: DrawListId, item: usize, at: usize },
    Uni { list: DrawListId, item: usize, at: usize },
    Scope { shader: usize, at: usize },
    Clear { pass: DrawPassId },
}

/// The live theme pulse: the colour, the current mix, the last value
/// written, and the ledger of every slot holding it. Slots are re-toned
/// by identity, never by value, so no other colour can be caught.
struct PulseState {
    target: [f32; 4],
    m: f32,
    last: [f32; 4],
    /// A theme edit: every slot holds this value instead of a pulse tone.
    fixed: Option<[f32; 4]>,
    ledger: Vec<PulseSlot>,
}

impl PulseState {
    fn new(rgba: u32) -> Self {
        let target = [
            ((rgba >> 24) & 0xff) as f32 / 255.0,
            ((rgba >> 16) & 0xff) as f32 / 255.0,
            ((rgba >> 8) & 0xff) as f32 / 255.0,
            (rgba & 0xff) as f32 / 255.0,
        ];
        Self { target, m: 0.0, last: target, fixed: None, ledger: Vec::new() }
    }
}

fn pulse_close(v: &[f32], w: [f32; 4]) -> bool {
    (v[0] - w[0]).abs() < 0.004
        && (v[1] - w[1]).abs() < 0.004
        && (v[2] - w[2]).abs() < 0.004
        && (v[3] - w[3]).abs() < 0.004
}

fn pulse_slot_get(cx: &Cx, s: PulseSlot) -> Option<[f32; 4]> {
    let v: [f32; 4] = match s {
        PulseSlot::Inst { list, item, at } => {
            let items = &cx.draw_lists[list].draw_items;
            if item >= items.len() {
                return None;
            }
            let b = items[item].instances.as_ref()?.get(at..at + 4)?;
            [b[0], b[1], b[2], b[3]]
        }
        PulseSlot::Uni { list, item, at } => {
            let items = &cx.draw_lists[list].draw_items;
            if item >= items.len() {
                return None;
            }
            let b = items[item].draw_call()?.dyn_uniforms.get(at..at + 4)?;
            [b[0], b[1], b[2], b[3]]
        }
        PulseSlot::Scope { shader, at } => {
            let b = cx.draw_shaders.shaders.get(shader)?.mapping.scope_uniforms_buf.get(at..at + 4)?;
            [b[0], b[1], b[2], b[3]]
        }
        PulseSlot::Clear { pass } => {
            let c = cx.passes[pass].clear_color;
            [c.x, c.y, c.z, c.w]
        }
    };
    Some(v)
}

fn pulse_slot_set(cx: &mut Cx, s: PulseSlot, v: [f32; 4]) {
    match s {
        PulseSlot::Inst { list, item, at } => {
            let items = &mut cx.draw_lists[list].draw_items;
            if item >= items.len() {
                return;
            }
            let it = &mut items[item];
            if let Some(dst) = it.instances.as_mut().and_then(|b| b.get_mut(at..at + 4)) {
                dst.copy_from_slice(&v);
            }
            if let Some(call) = it.kind.draw_call_mut() {
                call.instance_dirty = true;
            }
        }
        PulseSlot::Uni { list, item, at } => {
            let items = &mut cx.draw_lists[list].draw_items;
            if item >= items.len() {
                return;
            }
            if let Some(call) = items[item].kind.draw_call_mut() {
                if let Some(dst) = call.dyn_uniforms.get_mut(at..at + 4) {
                    dst.copy_from_slice(&v);
                    call.uniforms_dirty = true;
                }
            }
        }
        PulseSlot::Scope { shader, at } => {
            if let Some(sh) = cx.draw_shaders.shaders.get_mut(shader) {
                if let Some(dst) = sh.mapping.scope_uniforms_buf.get_mut(at..at + 4) {
                    dst.copy_from_slice(&v);
                    sh.mapping.scope_uniforms_gen = sh.mapping.scope_uniforms_gen.wrapping_add(1);
                }
            }
        }
        PulseSlot::Clear { pass } => {
            let p = &mut cx.passes[pass];
            p.clear_color = Vec4f { x: v[0], y: v[1], z: v[2], w: v[3] };
            if p.main_draw_list_id.is_some() {
                p.paint_dirty = true;
            }
        }
    }
}

/// Bring every draw buffer in line with the pulse: re-tone the ledger's
/// slots (dropping any a widget has since written something else into),
/// then adopt every fresh occurrence of the true colour — a redraw writes
/// the true values back, and this runs again after each one.
fn pulse_sync(cx: &mut Cx, st: &mut PulseState) -> usize {
    let pulsed = st.fixed.unwrap_or_else(|| pulse_tone(st.target, st.m));
    let target = st.target;
    let last = st.last;
    let live: std::collections::HashSet<DrawListId> = cx.draw_lists.id_iter().collect();
    let passes: Vec<DrawPassId> = cx.passes.id_iter().collect();
    let mut ledger = std::mem::take(&mut st.ledger);
    ledger.retain(|s| {
        match *s {
            PulseSlot::Inst { list, .. } | PulseSlot::Uni { list, .. } if !live.contains(&list) => return false,
            PulseSlot::Clear { pass } if !passes.contains(&pass) => return false,
            _ => {}
        }
        match pulse_slot_get(cx, *s) {
            Some(v) if pulse_close(&v, target) || pulse_close(&v, last) => {
                pulse_slot_set(cx, *s, pulsed);
                true
            }
            _ => false,
        }
    });
    let known: std::collections::HashSet<PulseSlot> = ledger.iter().copied().collect();
    let mut shader_slots: std::collections::HashMap<usize, (usize, Vec<usize>, Vec<usize>)> = Default::default();
    for list_id in live {
        let item_count = cx.draw_lists[list_id].draw_items.len();
        for item_id in 0..item_count {
            let Some(shader_index) = cx.draw_lists[list_id].draw_items[item_id]
                .draw_call()
                .map(|dc| dc.draw_shader_id.index)
            else {
                continue;
            };
            let (stride, inst_offs, uni_offs) = shader_slots
                .entry(shader_index)
                .or_insert_with(|| {
                    let mapping = &cx.draw_shaders.shaders[shader_index].mapping;
                    (
                        mapping.instances.total_slots,
                        mapping.instances.inputs.iter().filter(|i| i.slots == 4).map(|i| i.offset).collect(),
                        mapping.dyn_uniforms.inputs.iter().filter(|i| i.slots == 4).map(|i| i.offset).collect(),
                    )
                })
                .clone();
            let mut fresh: Vec<PulseSlot> = Vec::new();
            {
                let item = &cx.draw_lists[list_id].draw_items[item_id];
                if let (Some(buf), true) = (item.instances.as_ref(), stride > 0) {
                    for n in 0..buf.len() / stride {
                        for off in &inst_offs {
                            let at = n * stride + off;
                            let s = PulseSlot::Inst { list: list_id, item: item_id, at };
                            if at + 4 <= buf.len() && !known.contains(&s) && pulse_close(&buf[at..at + 4], target) {
                                fresh.push(s);
                            }
                        }
                    }
                }
                if let Some(call) = item.draw_call() {
                    for off in &uni_offs {
                        let s = PulseSlot::Uni { list: list_id, item: item_id, at: *off };
                        if *off + 4 <= call.dyn_uniforms.len()
                            && !known.contains(&s)
                            && pulse_close(&call.dyn_uniforms[*off..*off + 4], target)
                        {
                            fresh.push(s);
                        }
                    }
                }
            }
            for s in fresh {
                pulse_slot_set(cx, s, pulsed);
                ledger.push(s);
            }
        }
    }
    // Theme colours referenced inside shader code, and pass clear colours.
    let mut fresh: Vec<PulseSlot> = Vec::new();
    for shader in 0..cx.draw_shaders.shaders.len() {
        let mapping = &cx.draw_shaders.shaders[shader].mapping;
        for input in mapping.scope_uniforms.inputs.iter().filter(|i| i.slots == 4) {
            let at = input.offset;
            let s = PulseSlot::Scope { shader, at };
            if let Some(b) = mapping.scope_uniforms_buf.get(at..at + 4) {
                if !known.contains(&s) && pulse_close(b, target) {
                    fresh.push(s);
                }
            }
        }
    }
    for pass in passes {
        let s = PulseSlot::Clear { pass };
        let p = &cx.passes[pass];
        // A pass without a draw list never paints: nothing to see, and
        // marking it dirty only logs "Draw pass has no draw list!".
        if p.main_draw_list_id.is_none() {
            continue;
        }
        let c = p.clear_color;
        if !known.contains(&s) && pulse_close(&[c.x, c.y, c.z, c.w], target) {
            fresh.push(s);
        }
    }
    for s in fresh {
        pulse_slot_set(cx, s, pulsed);
        ledger.push(s);
    }
    st.last = pulsed;
    st.ledger = ledger;
    st.ledger.len()
}

/// Repaint (no redraw) what the pulse touched: the window passes, the
/// passes of every patched draw list, and every patched clear colour.
fn pulse_repaint(cx: &mut Cx, st: &PulseState) {
    let mut dirty: Vec<DrawPassId> = Vec::new();
    for pass in cx.passes.id_iter() {
        let p = &cx.passes[pass];
        if matches!(p.parent, CxDrawPassParent::Window(_)) && p.main_draw_list_id.is_some() {
            dirty.push(pass);
        }
    }
    for s in &st.ledger {
        match *s {
            PulseSlot::Inst { list, .. } | PulseSlot::Uni { list, .. } => {
                if let Some(pass) = cx.draw_lists[list].draw_pass_id {
                    dirty.push(pass);
                }
            }
            PulseSlot::Clear { pass } => dirty.push(pass),
            PulseSlot::Scope { .. } => {}
        }
    }
    for pass in dirty {
        cx.passes[pass].paint_dirty = true;
    }
}

/// Write the true colour back into every slot still holding the pulse.
fn pulse_restore(cx: &mut Cx, st: &PulseState) {
    for s in &st.ledger {
        if let Some(v) = pulse_slot_get(cx, *s) {
            if pulse_close(&v, st.last) {
                pulse_slot_set(cx, *s, st.target);
            }
        }
    }
}

/// The post-draw hook while a pulse is live: widgets that just redrew
/// wrote true colours; re-apply before the paint.
fn pulse_after_draw(cx: &mut Cx) {
    theme_overrides_sync(cx);
    let st = session().lock().unwrap().pulse.take();
    if let Some(mut st) = st {
        pulse_sync(cx, &mut st);
        session().lock().unwrap().pulse = Some(st);
    }
}

/// Re-apply every theme colour edit to the draw buffers (widgets that
/// redrew wrote their baked colour back).
fn theme_overrides_sync(cx: &mut Cx) {
    let mut overrides = std::mem::take(&mut session().lock().unwrap().theme_overrides);
    for (_, st) in overrides.iter_mut() {
        pulse_sync(cx, st);
    }
    session().lock().unwrap().theme_overrides = overrides;
}

/// The post-draw hook is up exactly while a pulse or a theme edit lives.
fn hook_sync(cx: &mut Cx) {
    let live = {
        let s = session().lock().unwrap();
        s.pulse.is_some() || !s.theme_overrides.is_empty()
    };
    cx.post_draw_hook = if live { Some(Box::new(pulse_after_draw)) } else { None };
}

/// One global theme value: a colour or a number.
#[derive(Clone, Copy)]
enum ThemeVal {
    Color(u32),
    Num(f64),
}

/// The app's theme: every colour and number in `mod.theme`, with the
/// level (file:line) that defines it. Read from the script cascade.
fn theme_values(cx: &mut Cx) -> Vec<(String, LiveId, ThemeVal, String)> {
    let mut out: Vec<(String, LiveId, ThemeVal, String)> = Vec::new();
    cx.with_vm(|vm| {
        let theme = vm.module(id!(theme));
        let chain = vm.construction_chain(theme.into());
        let mut keys: Vec<(LiveId, String)> = Vec::new();
        for lvl in &chain {
            let loc = lvl
                .loc
                .as_ref()
                .map(|l| format!("{}:{}", l.file.rsplit('/').next().unwrap_or(&l.file), l.line))
                .unwrap_or_default();
            for key in &lvl.own_keys {
                if !keys.iter().any(|(k, _)| k == key) {
                    keys.push((*key, loc.clone()));
                }
            }
        }
        for (key, loc) in keys {
            let name = live_id_token(key);
            let value = vm.bx.heap.value(theme, key.into(), NoTrap);
            if let Some(color) = value.as_color() {
                out.push((name, key, ThemeVal::Color(color), loc));
            } else if let Some(f) = value.as_f64() {
                out.push((name, key, ThemeVal::Num(f), loc));
            }
        }
    });
    out
}

/// The theme's colours (`color_*`), for the chips, the pulse and the strip.
fn theme_palette(cx: &mut Cx) -> Vec<(String, u32, String)> {
    theme_values(cx)
        .into_iter()
        .filter_map(|(name, _, value, loc)| match value {
            ThemeVal::Color(c) if name.starts_with("color") => Some((name, c, loc)),
            _ => None,
        })
        .collect()
}

/// Overwrite one theme value in the script heap, wherever in the theme's
/// prototype chain it is defined (the module is immutable to scripts).
fn theme_heap_set(cx: &mut Cx, key: LiveId, value: ScriptValue) -> bool {
    let mut hit = false;
    cx.with_vm(|vm| {
        let theme = vm.module(id!(theme));
        vm.proto_map_iter_mut_with(theme, &mut |_, map| {
            if let Some(entry) = map.get_mut(&key.into()) {
                entry.value = value;
                hit = true;
            }
        });
    });
    hit
}

fn rgba_of(c: u32) -> [f32; 4] {
    [
        ((c >> 24) & 0xff) as f32 / 255.0,
        ((c >> 16) & 0xff) as f32 / 255.0,
        ((c >> 8) & 0xff) as f32 / 255.0,
        (c & 0xff) as f32 / 255.0,
    ]
}

fn packed_of(rgba: [f32; 4]) -> u32 {
    ((rgba[0] * 255.0).round() as u32) << 24
        | ((rgba[1] * 255.0).round() as u32) << 16
        | ((rgba[2] * 255.0).round() as u32) << 8
        | ((rgba[3] * 255.0).round() as u32)
}

fn hex_of(c: u32) -> String {
    format_hex(rgba_of(c), true)
}

/// The theme rows' place in `rows_uid`: no widget, the theme itself.
const THEME_ROWS: u64 = u64::MAX;

/// Selected state by fill, never by brackets in the label.
fn set_button_fill(cx: &mut Cx, btn: WidgetRef, selected: bool) {
    let mut btn = btn;
    let color: Vec4f = if selected { vec4(0.31, 0.34, 0.44, 1.0) } else { vec4(0.20, 0.20, 0.21, 1.0) };
    script_apply_eval!(cx, btn, { draw_bg +: { color: #(color) } });
}

/// The selection's path for people: anonymous segments (`-`, list
/// indices) read as the widget's type, joined with ›.
fn display_path(cx: &Cx, uid: u64) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = Some(WidgetUid(uid));
    while let Some(u) = cur {
        let (name, parent, ty) = {
            let tree = cx.widget_tree();
            let name = tree.name_of(u).map(live_id_token).unwrap_or_else(|| "-".to_string());
            let ty = tree.widget(u).widget_type_id();
            (name, tree.parent_of(u), ty)
        };
        let anon = name == "-" || name.chars().all(|c| c.is_ascii_digit());
        let label = if anon {
            ty.and_then(|t| widget_type_names(cx).get(&t).copied())
                .map(live_id_token)
                .unwrap_or(name)
        } else {
            name
        };
        parts.push(label);
        cur = parent;
    }
    parts.reverse();
    parts.join(" \u{203a} ")
}

/// Keep the leaf visible: `…` then the last `keep` chars.
fn tail_ellipsis(s: &str, keep: usize) -> String {
    let n = s.chars().count();
    if n <= keep {
        return s.to_string();
    }
    let tail: String = s.chars().skip(n - keep).collect();
    format!("\u{2026}{tail}")
}

/// Which kind of place a cascade level comes from, for its icon:
/// 0 app file, 1 widget library file, 2 theme file, 3 native (no file).
fn cascade_icon_kind(file: &str) -> usize {
    if file.is_empty() {
        3
    } else if file.rsplit('/').next().unwrap_or("").contains("theme") {
        2
    } else if file.contains("widgets/src/") || file.contains("/widgets/") {
        1
    } else {
        0
    }
}

/// A history step's widget: the path when it round-trips, else the pinned
/// selection when the step was recorded against it (paths through
/// anonymous segments — `-`, list indices — never round-trip the finder,
/// and every undo of a scrub on such a widget silently did nothing).
fn resolve_widget_for_history(cx: &mut Cx, path: &str) -> Result<WidgetRef, String> {
    if let Ok(w) = resolve_widget_by_path(cx, path) {
        return Ok(w);
    }
    let pinned = session().lock().unwrap().pinned.clone();
    if let Some(p) = pinned {
        if p.path == path {
            let w = cx.widget_tree().widget(WidgetUid(p.uid));
            if !w.is_empty() {
                return Ok(w);
            }
        }
    }
    Err(format!("no widget at {path}"))
}

/// The compiled shader a widget's draw layer is drawing with, read
/// through the layer's live draw call (the primary material through the
/// widget's own area).
fn layer_shader_id(cx: &Cx, widget: &WidgetRef, layer: &str, primary: bool) -> Option<DrawShaderId> {
    let area = widget
        .layer_areas()
        .into_iter()
        .find(|(name, _)| *name == layer)
        .map(|(_, a)| a)
        .or_else(|| if primary { Some(widget.area()) } else { None })?;
    let Area::Instance(inst) = area else { return None };
    if inst.instance_count == 0 {
        return None;
    }
    let draw_list = &cx.draw_lists[inst.draw_list_id];
    if inst.draw_item_id >= draw_list.draw_items.len() {
        return None;
    }
    Some(draw_list.draw_items[inst.draw_item_id].draw_call()?.draw_shader_id)
}

/// The hot-patchable constants of one draw layer, as (index, name, doc,
/// initial, value, loc) — copied out so the caller may use cx freely.
fn layer_consts(cx: &mut Cx, widget: &WidgetRef, layer: &str, primary: bool) -> Vec<(DrawShaderId, usize, String, String, f32, f32, String)> {
    let mut out = Vec::new();
    let Some(shader) = layer_shader_id(cx, widget, layer, primary) else { return out };
    let raw: Vec<(usize, String, String, f32, f32, ScriptIp)> = cx
        .shader_const_table(shader)
        .iter()
        .enumerate()
        .map(|(i, tc)| (i, tc.name.clone(), tc.doc.clone(), tc.initial, tc.value, tc.ip))
        .collect();
    for (i, name, doc, initial, value, ip) in raw {
        // ip_to_loc names the literal's exact line (711a490ce fixed the
        // tokenizer's float-column measurement and script_mod!'s row 0).
        let loc = cx.with_vm(|vm| vm.bx.code.ip_to_loc(ip)).map(|l| {
            let base = l.file.rsplit('/').next().unwrap_or(&l.file).to_string();
            format!("{base}:{}", l.line)
        });
        out.push((shader, i, name, doc, initial, value, loc.unwrap_or_else(|| "?".to_string())));
    }
    out
}

/// A shader constant of the widget by name, across its draw layers.
fn const_lookup(cx: &mut Cx, widget: &WidgetRef, name: &str) -> Option<(DrawShaderId, usize, String, f32, f32)> {
    let mut layers: Vec<(String, bool)> = widget.layer_areas().into_iter().map(|(n, _)| (n.to_string(), false)).collect();
    if layers.is_empty() {
        layers.push(("draw_bg".to_string(), true));
    } else {
        layers[0].1 = true;
    }
    for (layer, primary) in layers {
        for (shader, i, cname, _doc, initial, value, loc) in layer_consts(cx, widget, &layer, primary) {
            if cname == name {
                return Some((shader, i, loc, initial, value));
            }
        }
    }
    None
}

/// Set (Some) or reset (None) a shader constant by name: the GPU takes it
/// next frame, no recompile, source untouched. Ledgered with the literal's
/// file:line and scope "shader" — every draw sharing that compiled shader
/// changes with it. Returns (old, new).
fn const_set(cx: &mut Cx, widget: &WidgetRef, path: &str, name: &str, value: Option<f64>, origin: &str) -> Result<(f32, f32), String> {
    let (shader, index, loc, initial, old) = const_lookup(cx, widget, name).ok_or_else(|| format!("no shader constant named {name:?} on this widget"))?;
    // Every site in the shader annotated with this name is the same knob.
    let sites: Vec<usize> = cx
        .shader_const_table(shader)
        .iter()
        .enumerate()
        .filter(|(_, tc)| tc.name == name)
        .map(|(i, _)| i)
        .collect();
    let sites = if sites.is_empty() { vec![index] } else { sites };
    // A settle that lands on the value already on the GPU (the Ended after
    // a scrub, a typed value equal to the current) is not an edit.
    let target = match value {
        Some(v) => v as f32,
        None => initial,
    };
    if target == old {
        return Ok((old, old));
    }
    let new = match value {
        Some(v) => {
            for i in &sites {
                if !cx.shader_const_patch(shader, *i, v as f32) {
                    return Err(format!("shader constant {name:?} could not be patched"));
                }
            }
            v as f32
        }
        None => {
            for i in &sites {
                cx.shader_const_reset(shader, *i);
            }
            initial
        }
    };
    let now = cx.seconds_since_app_start();
    let mut s = session().lock().unwrap();
    s.suppress_until = now + SUPPRESS_LINGER;
    s.apply_gen += 1;
    s.next_seq += 1;
    let entry = TweakDiffEntry {
        seq: s.next_seq,
        path: path.to_string(),
        prop: format!("const:{name}"),
        old: fmt_f64(old as f64),
        new: fmt_f64(new as f64),
        origin: format!("{loc} \u{00b7} shader {}: every widget drawing with it", shader.index),
        siblings: 0,
        scope: "shader".to_string(),
    };
    if origin != "undo" && origin != "redo" {
        log!("TWEAK {} {} {} {} -> {} ({})", origin, entry.path, entry.prop, entry.old, entry.new, entry.origin);
        track_undo(&mut s, &entry);
    }
    s.diff.push(entry);
    drop(s);
    cx.redraw_all();
    Ok((old, new))
}

/// How many component fields a typed editor has (the position of a field
/// uid modulo this is its component, whichever copy of the row it sits in).
fn comp_count(kind: StructKind) -> usize {
    match kind {
        StructKind::Vec2 => 2,
        StructKind::Vec3 => 3,
        StructKind::Vec4 | StructKind::Inset => 4,
        StructKind::Metrics => 3,
        _ => 1,
    }
}

/// The number token after `key:` in a `{..}` dump (`null` reads as 0).
fn struct_num(text: &str, key: &str) -> Option<f64> {
    let at = text.find(&format!("{key}:"))?;
    let rest = text[at + key.len() + 1..].trim_start();
    let tok: String = rest
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '}' && *c != ',')
        .collect();
    if tok == "null" {
        return Some(0.0);
    }
    tok.parse().ok()
}

/// Classify a reflected value dump into a typed editor + its component
/// values (empty for NoEditor). Only dumps reach here — scalars, colours
/// and bools are already their own row kinds.
fn parse_struct(value: &str) -> (StructKind, Vec<f64>) {
    let v = value.trim();
    for (pfx, k, n) in [
        ("vec2f(", StructKind::Vec2, 2usize),
        ("vec3f(", StructKind::Vec3, 3),
        ("vec4f(", StructKind::Vec4, 4),
    ] {
        if let Some(inner) = v.strip_prefix(pfx).and_then(|s| s.strip_suffix(')')) {
            let nums: Vec<f64> = inner.split_whitespace().filter_map(|s| s.parse().ok()).collect();
            if nums.len() == n {
                return (k, nums);
            }
        }
    }
    if v.starts_with('{') && v.ends_with('}') {
        if v.contains("left:") && v.contains("bottom:") {
            return (
                StructKind::Inset,
                vec![
                    struct_num(v, "left").unwrap_or(0.0),
                    struct_num(v, "top").unwrap_or(0.0),
                    struct_num(v, "right").unwrap_or(0.0),
                    struct_num(v, "bottom").unwrap_or(0.0),
                ],
            );
        }
        if v.contains("descender:") {
            return (
                StructKind::Metrics,
                vec![
                    struct_num(v, "descender").unwrap_or(0.0),
                    struct_num(v, "line_gap").unwrap_or(0.0),
                    struct_num(v, "line_scale").unwrap_or(1.0),
                ],
            );
        }
        if v.contains("min:") && v.contains("max:") {
            // A Size: Fill carries a weight, Fit does not (a fixed number
            // is a plain Num row, never a dump).
            return (StructKind::SizeField, vec![if v.contains("weight:") { 1.0 } else { 0.0 }]);
        }
        return (StructKind::NoEditor, Vec::new());
    }
    (StructKind::None, Vec::new())
}

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

/// `file:line` of the site that constructed the widget's source object —
/// the first level of its construction chain. A template instance answers
/// with the template's `:=` literal; a one-off widget with its own literal.
fn source_origin(cx: &mut Cx, widget: &WidgetRef) -> String {
    cascade_levels(cx, widget)
        .first()
        .filter(|l| !l.file.is_empty())
        .map(|l| format!("{}:{}", l.file, l.line))
        .unwrap_or_default()
}

/// The type's definition site: the deepest cascade level that has a file
/// location — `mod.widgets.Button = set_type_default() do …` in the widget
/// library — where an "all Buttons" edit belongs in source.
fn type_origin(cx: &mut Cx, widget: &WidgetRef) -> String {
    cascade_levels(cx, widget)
        .iter()
        .rev()
        .find(|l| !l.file.is_empty())
        .map(|l| format!("{}:{}", l.file, l.line))
        .unwrap_or_else(|| source_origin(cx, widget))
}

/// Every other live widget of the same widget type — "every Button in the
/// system".
fn type_siblings(cx: &mut Cx, widget: &WidgetRef) -> Vec<WidgetRef> {
    let Some(ty) = widget.widget_type_id() else {
        return Vec::new();
    };
    let me = widget.widget_uid();
    let rows = cx.widget_tree().flat_tree(cx);
    let mut out = Vec::new();
    for row in rows {
        if row.uid == me.0 {
            continue;
        }
        let other = cx.widget_tree().widget(WidgetUid(row.uid));
        if other.is_empty() || other.widget_type_id() != Some(ty) {
            continue;
        }
        // The panel's own buttons/labels are the same types as the app's;
        // "all Button" means the app's buttons.
        let in_tweaker = cx
            .widget_tree()
            .path_to(WidgetUid(row.uid))
            .iter()
            .any(|id| *id == live_id!(tweaker));
        if in_tweaker {
            continue;
        }
        out.push(other);
    }
    out
}

/// Every other live widget built from the same source object (the same
/// template): the tabs beside a tab, the rows beside a list row.
fn template_siblings(cx: &mut Cx, widget: &WidgetRef) -> Vec<WidgetRef> {
    let source = widget.script_source();
    if source == ScriptObject::ZERO {
        return Vec::new();
    }
    let me = widget.widget_uid();
    let rows = cx.widget_tree().flat_tree(cx);
    let mut out = Vec::new();
    for row in rows {
        if row.uid == me.0 {
            continue;
        }
        let other = cx.widget_tree().widget(WidgetUid(row.uid));
        if other.is_empty() || other.script_source() != source {
            continue;
        }
        out.push(other);
    }
    out
}

/// Values survive an apply (they live in the Rust draw vars) but the
/// SHADER is recomputed from the chain of the object just applied — and
/// every chunk derives a fresh object from the widget's untouched source,
/// so `draw_bg +: {color: #00f}` after a live `pixel: fn` edit compiled the
/// file's pixel back in (the fn edit vanished on the next colour tweak).
/// After a chunk that touches a layer with live fn edits, put those fns
/// back on top; their text is unchanged, so the compiled shader is a cache
/// hit.
fn reinject_fn_overrides(cx: &mut Cx, widget: &WidgetRef, chunk: &str) {
    let uid = widget.widget_uid().0;
    let overrides: Vec<((u64, String, String), String)> = session()
        .lock()
        .unwrap()
        .fn_overrides
        .iter()
        .filter(|((owner, _, _), _)| *owner == uid)
        .map(|(key, text)| (key.clone(), text.clone()))
        .collect();
    if overrides.is_empty() {
        return;
    }
    let mut layers: Vec<String> = Vec::new();
    for ((_, layer, name), _) in &overrides {
        let touches_layer = chunk.contains(&format!("{layer} +:"))
            || chunk.contains(&format!("{layer}:"))
            || chunk.contains(&format!("{layer}."));
        let defines_fn = chunk.contains(&format!("{name}:"));
        if touches_layer && !defines_fn && !layers.contains(layer) {
            layers.push(layer.clone());
        }
    }
    for layer in layers {
        let mut body = String::new();
        for ((_, l, _), text) in &overrides {
            if *l == layer {
                body.push_str(text);
                body.push('\n');
            }
        }
        let _ = makepad_platform::shader_error::take();
        if let Err(error) = eval_chunk(cx, widget, &format!("{layer} +: {{\n{body}}}")) {
            log!("TWEAK re-applying the live {layer} fns failed: {error}");
        }
        if let Some(error) = makepad_platform::shader_error::take() {
            log!("TWEAK re-applied live {layer} fns did not compile: {}", terse_shader_error(&error));
        }
    }
}

/// The first problem of a draw-shader compile report, without the
/// compiler's own source pointers; says how many more there were.
fn terse_shader_error(report: &str) -> String {
    let lines: Vec<&str> = report.lines().filter(|l| !l.trim().is_empty()).collect();
    let first = lines.first().copied().unwrap_or(report);
    // `DrawQuad: shader field …` — the type name is the layer's, keep it.
    let first = match first.find(" (platform/") {
        Some(at) => &first[..at],
        None => first,
    };
    let first: String = first.chars().take(160).collect();
    if lines.len() > 1 {
        format!("{first} (+{} more)", lines.len() - 1)
    } else {
        first
    }
}

/// Undo a chunk whose shader failed to compile: an fn chunk puts the
/// layer's fns back (the last text applied from the view or the AI, else
/// the fn as written); a single-property chunk puts the property back.
fn revert_failed_chunk(cx: &mut Cx, widget: &WidgetRef, chunk: &str, before: &[(String, String, bool)]) {
    if chunk.contains("fn") {
        if let Some(open) = chunk.find("+:") {
            let layer = chunk[..open].trim().to_string();
            revert_layer_fns(cx, widget, &layer);
            return;
        }
    }
    if let Some((prop, _)) = single_prop_chunk(chunk) {
        if let Some((_, old, quoted)) = before.iter().find(|(name, _, _)| *name == prop) {
            let text = if *quoted { format!("{old:?}") } else { old.clone() };
            let _ = makepad_platform::shader_error::take();
            if let Err(error) = eval_chunk(cx, widget, &format!("{prop}: {text}")) {
                log!("TWEAK revert of {prop} failed: {error}");
            }
            let _ = makepad_platform::shader_error::take();
        }
    }
}

fn revert_layer_fns(cx: &mut Cx, widget: &WidgetRef, layer: &str) {
    let uid = widget.widget_uid().0;
    let sources = layer_fn_sources(cx, widget, layer);
    let overrides = session().lock().unwrap().fn_overrides.clone();
    let mut body = String::new();
    for (name, _loc, src) in &sources {
        let text = overrides
            .get(&(uid, layer.to_string(), name.clone()))
            .cloned()
            .unwrap_or_else(|| src.clone());
        body.push_str(&text);
        body.push('\n');
    }
    if body.trim().is_empty() {
        return;
    }
    let _ = makepad_platform::shader_error::take();
    if let Err(error) = eval_chunk(cx, widget, &format!("{layer} +: {{\n{body}}}")) {
        log!("TWEAK revert of {layer} fns failed: {error}");
    }
    if let Some(error) = makepad_platform::shader_error::take() {
        log!("TWEAK revert of {layer} fns did not compile either: {}", terse_shader_error(&error));
    }
}

/// Rewrite every `tweak://apply:LINE:COL` in an error into the chunk's own
/// line numbers.
fn relocate_chunk_error(error: &str, callsite: u32) -> String {
    let mut out = String::new();
    let mut rest = error;
    while let Some(at) = rest.find("tweak://apply:") {
        out.push_str(&rest[..at]);
        let tail = &rest[at + "tweak://apply:".len()..];
        let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
        match digits.parse::<u32>() {
            Ok(line) if line > callsite => {
                // The parser counts the `use` line and one more for the
                // wrapper: measured, `@@` on chunk line 4 reports callsite+5.
                out.push_str(&format!("line {}", line - callsite - 1));
                rest = &tail[digits.len()..];
            }
            _ => {
                out.push_str("tweak://apply:");
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A stable pseudo-line for a chunk: FNV-1a of its text, so each distinct
/// chunk is its own script body (and its own fn ScriptIps).
fn chunk_callsite_line(code: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in code.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    (h % 1_000_000) + 2
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
    let callsite = chunk_callsite_line(&code);
    let errors = cx.with_vm(|vm| {
        // Install a captured-error sink so parse/apply problems come back to
        // the caller instead of only landing in the log.
        vm.bx.captured_errors = Some(Vec::new());
        let script_mod = ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: "tweak".to_string(),
            // The callsite is keyed by the chunk's own text: the draw-shader
            // cache hashes each fn's ScriptIp, and one fixed callsite reused
            // its body across applies — same ip, same hash, so the SECOND
            // `pixel: fn` rewrite on a widget was recorded everywhere and
            // never recompiled. Identical chunks still dedup to one body.
            file: "tweak://apply".to_string(),
            line: callsite as usize,
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
        // `tweak://apply:<callsite+n>:<col>` → `line <n>:<col>` of the chunk
        // (chunk line 1 sits on code line 2, right after the `use`).
        let errors: Vec<String> = errors
            .iter()
            .map(|e| relocate_chunk_error(e, callsite))
            .collect();
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
    // The draw shader compiles inside the apply itself, so an fn that names
    // a field this layer does not have fails RIGHT HERE. Never answer ok
    // with a widget that stopped drawing: put the layer back and say why.
    let _ = makepad_platform::shader_error::take();
    eval_chunk(cx, widget, chunk)?;
    if let Some(error) = makepad_platform::shader_error::take() {
        revert_failed_chunk(cx, widget, chunk, &before);
        let terse = terse_shader_error(&error);
        log!("TWEAK {} {} rejected, shader did not compile: {}", origin, path, terse);
        return Err(format!("shader compile error: {terse}"));
    }
    // Template-built widgets (tabs, list items) share one source object;
    // an edit to one is an edit to the template, so every live sibling
    // takes it too — and the ledger names the template's site as the
    // place to change in source.
    // Scope decides who else takes the edit and which site the ledger
    // names: "this" = template siblings only (a tab is every tab) and the
    // instance's own site; "all" = every live widget of the TYPE and the
    // type's definition site.
    let scope_all = session().lock().unwrap().scope_all;
    let scope_name = if scope_all { "all" } else { "this" };
    let origin_site = if scope_all {
        type_origin(cx, widget)
    } else {
        source_origin(cx, widget)
    };
    let fan_out = if scope_all {
        type_siblings(cx, widget)
    } else {
        template_siblings(cx, widget)
    };
    reinject_fn_overrides(cx, widget, chunk);
    let mut siblings = 0u32;
    for sibling in fan_out {
        if eval_chunk(cx, &sibling, chunk).is_ok() {
            siblings += 1;
            reinject_fn_overrides(cx, &sibling, chunk);
        }
    }

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
        // A `prop: theme.color_x` chunk ledgers the REFERENCE, not the hex
        // the reflection resolves it to — the AI writes the reference into
        // source.
        let theme_ref = single_prop_chunk(chunk)
            .filter(|(_, value)| value.trim().starts_with("theme."))
            .map(|(prop, value)| (prop, value.trim().to_string()));
        for (name, new_value, _) in &after {
            let old_value = before
                .iter()
                .find(|(old_name, _, _)| old_name == name)
                .map(|(_, value, _)| value.clone())
                .unwrap_or_else(|| "-".to_string());
            if &old_value != new_value {
                s.next_seq += 1;
                let shown_new = match &theme_ref {
                    Some((p, reference)) if p == name => reference.clone(),
                    _ => new_value.clone(),
                };
                let entry = TweakDiffEntry {
                    seq: s.next_seq,
                    path: path.to_string(),
                    prop: name.clone(),
                    old: old_value,
                    new: shown_new,
                    origin: origin_site.clone(),
                    siblings,
                    scope: scope_name.to_string(),
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
                if origin != "undo" && origin != "redo" {
                    track_undo(&mut s, &entry);
                }
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
                        origin: origin_site.clone(),
                        siblings,
                        scope: scope_name.to_string(),
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
                        origin: origin_site.clone(),
                        siblings,
                        scope: scope_name.to_string(),
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
            if origin != "undo" && origin != "redo" {
                track_undo(&mut s, &entry);
            }
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
            "{{\"path\":{},\"prop\":{},\"old\":{},\"new\":{},\"origin\":{},\"siblings\":{},\"scope\":{}}}",
            json_str(&entry.path),
            json_str(&entry.prop),
            json_str(&entry.old),
            json_str(&entry.new),
            json_str(&entry.origin),
            entry.siblings,
            json_str(&entry.scope)
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
        // TEMP DEBUG RIG (do not commit): /tweak/op?op=perf — enable the
        // perf monitor and dump ring averages for framerate diagnosis.
        "perf" => {
            if !cx.perf_monitor.enabled() {
                cx.perf_monitor.set_enabled(true);
                return Ok("{\"enabled\":1,\"note\":\"call again for data\"}".to_string());
            }
            let mut frames = Vec::new();
            cx.perf_monitor.read(&mut frames);
            let live: Vec<_> = frames.iter().filter(|f| f.gap_ms > 0.0).collect();
            let n = live.len().max(1) as f32;
            let avg_gap: f32 = live.iter().map(|f| f.gap_ms).sum::<f32>() / n;
            let max_gap: f32 = live.iter().map(|f| f.gap_ms).fold(0.0, f32::max);
            let mut sorted: Vec<f32> = live.iter().map(|f| f.gap_ms).collect();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let p95 = sorted.get(sorted.len().saturating_sub(1) * 95 / 100).copied().unwrap_or(0.0);
            let mut out = format!(
                "{{\"frames_painted\":{},\"ring_frames\":{},\"gap_avg_ms\":{:.2},\"gap_p95_ms\":{:.2},\"gap_max_ms\":{:.2},\"channels\":{{",
                cx.perf_monitor.frames_painted(), live.len(), avg_gap, p95, max_gap
            );
            let names: Vec<String> = cx.perf_monitor.channels().iter().map(|c| c.name.clone()).collect();
            for (i, name) in names.iter().enumerate() {
                if i > 0 { out.push(','); }
                let avg_us: f32 = live.iter().map(|f| f.channel_us[i] as f32).sum::<f32>() / n;
                let max_us = live.iter().map(|f| f.channel_us[i]).max().unwrap_or(0);
                out.push_str(&format!("{}:{{\"avg_us\":{:.0},\"max_us\":{}}}", json_str(name), avg_us, max_us));
            }
            let ts = cx.widget_tree().stats();
            out.push_str(&format!(
                "}},\"tree\":{{\"lookups\":{},\"misses\":{},\"walk_nodes\":{},\"invalidations\":{},\"stores_skipped\":{}}}}}",
                ts.lookups, ts.cache_misses, ts.walk_nodes, ts.invalidations, ts.stores_skipped
            ));
            Ok(out)
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
            if let Some(r) = session().lock().unwrap().popup {
                out.push_str(&format!(",\"popup\":[{},{},{},{}]", r.pos.x, r.pos.y, r.size.x, r.size.y));
            }
            if let Some(pick) = &pinned {
                out.push_str(",\"sel\":");
                out.push_str(&pick_json(pick));
                // Resolve by UID first: paths with anonymous numeric
                // segments (a list item's `demos.1` Slider) do not
                // round-trip through the path finder, but the uid is
                // always exact for the pinned selection.
                let widget = {
                    let by_uid = cx.widget_tree().widget(WidgetUid(pick.uid));
                    if by_uid.is_empty() {
                        resolve_widget_by_path(cx, &pick.path).ok()
                    } else {
                        Some(by_uid)
                    }
                };
                if let Some(widget) = widget {
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
            {
                let notes = session().lock().unwrap().notes.clone();
                if !notes.is_empty() {
                    out.push_str(",\"notes\":[");
                    for (i, note) in notes.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        out.push_str(&format!(
                            "{{\"path\":{},\"text\":{}}}",
                            json_str(&note.path),
                            json_str(&note.text)
                        ));
                    }
                    out.push(']');
                }
            }
            {
                let vibes = session().lock().unwrap().vibes.clone();
                if !vibes.is_empty() {
                    out.push_str(",\"vibe\":[");
                    for (i, (vpath, vlayer, vprompt, vfns)) in vibes.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        out.push_str(&format!(
                            "{{\"path\":{},\"layer\":{},\"prompt\":{},\"fns\":{}}}",
                            json_str(vpath),
                            json_str(vlayer),
                            json_str(vprompt),
                            json_str(vfns)
                        ));
                    }
                    out.push(']');
                }
            }
            out.push_str(",\"diff\":");
            out.push_str(&diff_json(&diff));
            out.push_str(",\"ann\":");
            out.push_str(&strokes_json(&strokes));
            // The repaint counter, read WITHOUT causing a frame: two reads
            // apart in time tell whether the window animates on its own
            // (a shader that reads draw_pass.time keeps every live pass
            // repainting — the platform's time repaint, the spinner's too).
            out.push_str(&format!(",\"f\":{}", cx.repaint_id()));
            out.push_str(&format!(",\"shaders\":{}", cx.draw_shaders.shaders.len()));
            {
                let s = session().lock().unwrap();
                out.push_str(",\"states\":[");
                out.push_str(&s.state_names.iter().map(|n| json_str(n)).collect::<Vec<_>>().join(","));
                out.push(']');
                out.push_str(&format!(
                    ",\"states_lock\":{}",
                    s.states_lock.map_or("null".to_string(), |v| format!("{v}"))
                ));
            }
            out.push('}');
            Ok(out)
        }
        "undo" | "redo" => {
            session().lock().unwrap().undo_redo = Some(op == "undo");
            cx.redraw_all();
            Ok(format!("{{\"ok\":1,\"op\":{}}}", json_str(op)))
        }
        "theme" => {
            // Set one global theme value: name=color_x&value=#hex, or a number.
            let name = arg(args, &["name"]).unwrap_or("").to_string();
            let value = arg(args, &["value"]).unwrap_or("").to_string();
            if value.is_empty() {
                // No value: report the theme's current one.
                let current = theme_values(cx).into_iter().find(|(n, _, _, _)| *n == name).map(|(_, _, v, _)| match v {
                    ThemeVal::Color(c) => hex_of(c),
                    ThemeVal::Num(f) => fmt_f64(f),
                });
                return Ok(format!(
                    "{{\"ok\":1,\"theme\":{},\"value\":{}}}",
                    json_str(&name),
                    current.map_or("null".to_string(), |v| json_str(&v))
                ));
            }
            session().lock().unwrap().theme_req = Some((name.clone(), value.clone()));
            cx.redraw_all();
            Ok(format!("{{\"ok\":1,\"theme\":{},\"value\":{}}}", json_str(&name), json_str(&value)))
        }
        "pulse" => {
            // Pin the theme pulse on a colour (name=color_x or a #hex);
            // an empty name restores and unpins.
            let name = arg(args, &["name", "color"]).unwrap_or("").to_string();
            let lock = arg(args, &["m"]).and_then(|s| s.parse::<f32>().ok()).map(|m| m.clamp(0.0, 1.0));
            {
                let mut s = session().lock().unwrap();
                s.pulse_req = Some(name.clone());
                s.pulse_lock = lock;
            }
            cx.redraw_all();
            Ok(format!(
                "{{\"ok\":1,\"pulse\":{},\"m\":{}}}",
                json_str(&name),
                lock.map_or("null".to_string(), |m| format!("{m}"))
            ))
        }
        "states" => {
            // The state swatches' pose lock: phase=0 (off pose), 1 (on
            // pose), any mix between, or auto to animate again.
            let phase = arg(args, &["phase"]).map(|s| s.to_string());
            let mut s = session().lock().unwrap();
            match phase.as_deref() {
                Some("auto") => s.states_lock = None,
                Some(p) => {
                    if let Ok(v) = p.parse::<f64>() {
                        s.states_lock = Some(v.clamp(0.0, 1.0));
                    }
                }
                None => {}
            }
            let lock = s.states_lock;
            let names = s.state_names.clone();
            drop(s);
            cx.redraw_all();
            Ok(format!(
                "{{\"ok\":1,\"lock\":{},\"states\":[{}]}}",
                lock.map_or("null".to_string(), |v| format!("{v}")),
                names.iter().map(|n| json_str(n)).collect::<Vec<_>>().join(",")
            ))
        }
        "apply" => {
            if !tweak_is_on() {
                set_tweak_on(cx, true);
            }
            let path = arg(args, &["path", "p"]).ok_or("need path=")?.to_string();
            let chunk = match arg(args, &["splash", "s", "chunk"]) {
                Some(chunk) => chunk.to_string(),
                None if arg(args, &["const"]).is_some() => String::new(),
                None => {
                    let prop = arg(args, &["prop"]).ok_or("need splash= or prop=+value=")?;
                    let value = arg(args, &["value", "v"]).ok_or("need value=")?;
                    format!("{prop}: {value}")
                }
            };
            let widget = {
                // Anonymous path segments (`-`, list indices) do not round-trip
                // the path finder; the pinned selection resolves by uid, and a
                // caller may pass uid= outright.
                let by_uid = arg(args, &["uid"]).and_then(|s| s.parse::<u64>().ok());
                let pinned = session().lock().unwrap().pinned.clone();
                let w = match (by_uid, pinned) {
                    (Some(uid), _) => cx.widget_tree().widget(WidgetUid(uid)),
                    (None, Some(p)) if p.path == path => cx.widget_tree().widget(WidgetUid(p.uid)),
                    _ => WidgetRef::empty(),
                };
                if w.is_empty() {
                    resolve_widget_by_path(cx, &path)?
                } else {
                    w
                }
            };
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
            // A shader constant by name: hot-patched on the GPU (reset=1 puts
            // the literal back). No chunk, no recompile.
            if let Some(cname) = arg(args, &["const"]).map(|s| s.to_string()) {
                let reset = arg(args, &["reset"]).is_some_and(|r| r == "1" || r == "true");
                let value = if reset {
                    None
                } else {
                    Some(
                        arg(args, &["value", "v"])
                            .ok_or("need value= (or reset=1)")?
                            .trim()
                            .parse::<f64>()
                            .map_err(|_| "value= must be a number".to_string())?,
                    )
                };
                let (old, new) = const_set(cx, &widget, &resolved_path, &cname, value, "remote")?;
                session().lock().unwrap().pinned = Some(TweakPick {
                    uid: widget.widget_uid().0,
                    path: resolved_path.clone(),
                    ty: String::new(),
                    rect: widget.area().clipped_rect_union(cx),
                    window_id: 0,
                    band: None,
                    level: 0,
                });
                return Ok(format!(
                    "{{\"ok\":1,\"path\":{},\"const\":{},\"old\":{},\"new\":{}}}",
                    json_str(&resolved_path),
                    json_str(&cname),
                    fmt_f64(old as f64),
                    fmt_f64(new as f64)
                ));
            }
            let applied = apply_splash_chunk(cx, &widget, &resolved_path, &chunk, "remote");
            {
                // The prompt's answer landed (or failed): say so in the panel.
                let mut s = session().lock().unwrap();
                if let Some((ppath, _)) = s.vibe_pending.clone() {
                    if ppath == resolved_path || ppath == path {
                        s.vibe_status = match &applied {
                            Ok(_) => "applied \u{2713}".to_string(),
                            Err(e) => format!("error: {e}"),
                        };
                        s.vibe_pending = None;
                    }
                }
            }
            cx.redraw_all();
            let changed = applied?;
            // A fn rewrite from the AI shows in the source view as applied.
            record_fn_overrides(widget.widget_uid().0, &chunk);
            {
                let mut s = session().lock().unwrap();
                let rect = widget.area().clipped_rect_union(cx);
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
                    level: 0,
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

    set_type_default() do #(DrawTweakChecker::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            // The alpha-visualizing checkerboard: 6px two-tone squares.
            let p = floor(self.pos * self.rect_size / 6.0)
            let t = modf(p.x + p.y, 2.0)
            return mix(vec4(0.42, 0.42, 0.42, 1.0), vec4(0.58, 0.58, 0.58, 1.0), t)
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

    mod.widgets.TweakMaterialSwatchBase = #(TweakMaterialSwatch::register_widget(vm))
    mod.widgets.TweakMaterialSwatch = set_type_default() do mod.widgets.TweakMaterialSwatchBase{
        width: Fill
        height: 34
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

/// A live material preview: a quad drawn with the SELECTION'S OWN draw
/// shader. The tweaker applies the selected widget's current draw-layer
/// value onto `preview` (same source, same instance values), so the
/// fn-hash shader cache compiles to the identical shader — the swatch IS
/// the material, not a color approximation. Quad-family layers only
/// (guarded by a rect_pos probe); other layers draw the default flat quad.
#[derive(Script, ScriptHook, Widget)]
pub struct TweakMaterialSwatch {
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
    preview: DrawQuad,
    #[live]
    checker: DrawTweakChecker,
    /// The widget whose material this swatch mirrors (0 = none): the well
    /// re-reads that widget's live draw call every time it draws, so a
    /// scrub on the widget shows in the well the same frame.
    #[rust]
    mirror_uid: u64,
    #[rust]
    mirror_layer: String,
    /// True when `mirror_layer` is the widget's primary material — the one
    /// its area belongs to (draw_bg for most, draw_slider for a Slider…).
    #[rust]
    mirror_primary: bool,
    /// The well's own draw list: the mirrored instance is drawn at the
    /// widget's NATIVE size (so px-sized shader internals — a checkbox's
    /// 14px mark box, a border width — stay true) and the list's view
    /// transform magnifies it into the well. A magnifier, not a stretch.
    #[rust]
    well_list: Option<DrawList2d>,
    #[rust]
    area: Area,
    /// Which draw layer this swatch currently previews.
    #[rust]
    pub layer: String,
    /// Rebuild generation the preview was last applied at (MAX = never).
    #[rust(u64::MAX)]
    pub applied_gen: u64,
    /// The scroll viewport this swatch lives in (screen coords): the
    /// mirrored draw call is clipped to it (its own draw list escapes the
    /// scroll view's clipping otherwise).
    #[rust]
    pub clip: Option<Rect>,
    /// When set, the swatch shows ONE animator track of the mirrored
    /// widget: the layer's instance slice is posed between the track's off
    /// and on apply values by `mix` (0 = off, 1 = on) before it draws.
    #[rust]
    pub state: Option<StateTrack>,
    #[rust]
    pub mix: f32,
}

impl Widget for TweakMaterialSwatch {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let turtle_rect = cx.turtle().rect();
        let aligned = self.checker.area().rect(cx);
        let rect = if aligned.size.x > 0.0 && aligned.size.y > 0.0 { aligned } else { turtle_rect };
        self.checker.draw_abs(cx, turtle_rect);
        // Same shader, same inputs: copy the widget's live draw call (its
        // shader id, uniforms, textures and this instance's values —
        // colours, border sizes, hover/focus/down mix factors as they are
        // right now) and draw one instance of it here with only the
        // geometry substituted. "you do kinda have to feed it similar
        // inputs as the actual widget probably like colors and things".
        let mirrored = if self.mirror_uid != 0 {
            let widget = cx.widget_tree().widget(WidgetUid(self.mirror_uid));
            // The layer's own area when the widget exposes it (every
            // `#[live] Draw…` field does), else the widget's area for its
            // primary material.
            let layer_area = widget
                .layer_areas()
                .into_iter()
                .find(|(name, _)| *name == self.mirror_layer)
                .map(|(_, a)| a)
                .or_else(|| if self.mirror_primary { Some(widget.area()) } else { None });
            layer_area.and_then(|area| capture_material_mirror(cx, &widget, area, &self.preview.draw_vars))
        } else {
            None
        };
        match mirrored {
            Some(mut mirror) => {
                if let Some(state) = &self.state {
                    state.pose(cx, &mut mirror, self.mix);
                }
                let native = mirror.native_size();
                if self.well_list.is_none() {
                    self.well_list = Some(DrawList2d::new(cx));
                }
                let list = self.well_list.as_mut().unwrap();
                // The swatch only draws when its panel redraws, and every such
                // draw is a new capture: the well list must redraw with it, or
                // it keeps showing the previous selection's material.
                let well_id = list.id();
                cx.redraw_list(well_id);
                if list.begin(cx, Walk::abs_rect(rect)).is_redrawing() {
                    // Integer zoom when it fits, a shrink when it does not.
                    let fit = (rect.size.x / native.x.max(1.0)).min(rect.size.y / native.y.max(1.0));
                    let k = if fit >= 1.0 { fit.floor().min(8.0) } else { fit };
                    let tx = rect.pos.x + (rect.size.x - native.x * k) * 0.5;
                    let ty = rect.pos.y + (rect.size.y - native.y * k) * 0.5;
                    let m = Mat4f {
                        v: [
                            k as f32, 0.0, 0.0, 0.0, //
                            0.0, k as f32, 0.0, 0.0, //
                            0.0, 0.0, 1.0, 0.0, //
                            tx as f32, ty as f32, 0.0, 1.0,
                        ],
                    };
                    let id = list.id();
                    cx.draw_lists[id].draw_list_uniforms.view_transform = m;
                    // The scroll viewport's clip, seen through the
                    // magnifier: (screen - t) / k.
                    let clip = self.clip.map(|c| {
                        (
                            dvec2((c.pos.x - tx) / k, (c.pos.y - ty) / k),
                            dvec2((c.pos.x + c.size.x - tx) / k, (c.pos.y + c.size.y - ty) / k),
                        )
                    });
                    let visible = clip.map_or(true, |(min, max)| max.x > min.x.max(0.0) && max.y > min.y.max(0.0) && min.x < native.x && min.y < native.y);
                    if visible {
                        mirror.draw(cx, Rect { pos: dvec2(0.0, 0.0), size: native }, clip);
                    }
                    list.end(cx);
                }
            }
            None => self.preview.draw_abs(cx, rect),
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

/// The checkerboard behind material swatches: translucent shaders read
/// against it (the standard alpha backdrop).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTweakChecker {
    #[deref]
    draw_super: DrawQuad,
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
            SectionKind::Layout => "Layout",
            SectionKind::Style => "Style",
            SectionKind::Text => "Text",
            SectionKind::Behavior => "Behavior",
            SectionKind::Other => "Other",
            SectionKind::Cascade => "Cascade",
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
    /// A structured value's typed editor (vec/inset/metrics/size); None for
    /// scalar rows.
    struct_kind: StructKind,
    /// Structured value's live component values (vec x/y/z/w, inset legs…).
    comp_vals: Vec<f64>,
    /// The uids of the component number fields, in component order.
    comp_uids: Vec<u64>,
    /// The uids of a SizeField's Fill/Fit buttons ([fill, fit]).
    mode_uids: Vec<u64>,
    /// Field uids of this row's top-section copy: (uid, is_swatch).
    alt_uids: Vec<(u64, bool)>,
    /// A shader-constant row: an annotated literal inside a draw layer's
    /// fn body (Cx::shader_const_table), hot-patched on the GPU — never a
    /// chunk apply.
    const_ref: Option<ConstRef>,
    /// The theme colour this row's value equals, when one does.
    theme_match: Option<String>,
    /// The row's "≈ theme.color_x" button uid (click = use the reference).
    theme_uid: u64,
}

/// One hot-patchable shader constant of the pinned widget's draw layer.
#[derive(Clone)]
struct ConstRef {
    layer: String,
    name: String,
    initial: f32,
}

/// One visible sidebar entry, with the rects the raw-pointer gestures
/// (section fold, label double-click reset) hit-test against.
/// The doc tooltip a hovered row shows (text + pointer position).
#[derive(Clone, PartialEq)]
struct HoverDoc {
    text: String,
    pos: Vec2d,
}

/// The side panel's tabs.
#[derive(Clone, Copy, PartialEq, Default)]
enum PanelTab {
    #[default]
    Props,
    Shader,
    Tree,
    /// The global theme: its colours, spacing and font sizes, edited live.
    Theme,
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
    /// A material card header: layer name + live shader preview swatch
    /// (index into Tweaker::materials).
    Material(usize),
    /// The TWEAKABLES header at the top: every annotated value, hottest
    /// first, with its doc under it — the designer's surface.
    TweakHeader(usize, bool),
    /// An annotated row rendered inside TWEAKABLES (same template as Prop;
    /// the row keeps a second set of field uids for it).
    Tweakable(usize),
    /// The annotation line (doc + range) under a row.
    /// The Shader tab's INPUTS header: the mirrored layer's own inputs.
    InputsHeader(usize),
    /// One level of the selection's cascade (index into Tweaker::cascade).
    CascadeLevel(usize),
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
    /// The selection's draw layers, in row order (material card headers).
    #[rust]
    materials: Vec<String>,
    /// Doc-channel text per row prop (tooltips + scrubber hints).
    #[rust]
    row_docs: HashMap<String, String>,
    /// The doc tooltip shown on row hover.
    #[rust]
    hover_doc: Option<HoverDoc>,
    /// Which cascade level (0 = instance) set each top-level prop.
    #[rust]
    origin_levels: HashMap<String, usize>,
    /// The selection's construction chain, one entry per level.
    #[rust]
    cascade: Vec<CascadeLevel>,
    /// How many cascade levels the selection has (colors + scroll target).
    #[rust]
    cascade_level_count: usize,
    /// Focus the filter input on the next sidebar draw ('/' or tweak-on).
    #[rust]
    focus_search_pending: bool,
    /// The exploded-view toggle button beside the filter.
    #[rust]
    sploded_uid: u64,
    /// The exploded view's level-separation scrub field (visible only
    /// while the mode is up).
    spread_uid: u64,
    /// The panel's note button: the same note as the Insert key, for
    /// keyboards without one.
    note_uid: u64,
    /// The scope toggle's buttons.
    #[rust]
    scope_this_uid: u64,
    #[rust]
    scope_all_uid: u64,
    /// The Shader tab's editor and the live-code loop: every change arms a
    /// short debounce; at settle the fn text is applied through the ledger
    /// (one entry per settle, like a scrub gesture). A compile error keeps
    /// the last good text running and shows the message under the editor.
    #[rust]
    shader_src_uid: u64,
    /// The source editor stays folded until asked for: the Shader tab
    /// opens on the swatch + doc + prompt, and this button uid toggles the
    /// source view open.
    #[rust]
    shader_fold_uid: u64,
    #[rust]
    shader_src_open: bool,
    #[rust]
    live_timer: Timer,
    #[rust]
    live_last_applied: String,
    #[rust]
    live_last_good: String,
    #[rust]
    live_error_pending: bool,
    /// (widget, layer) the source view currently holds text for.
    #[rust]
    live_key: (u64, String),
    /// The layer doc in full when the terse line had to cut it.
    #[rust]
    shader_doc_full: String,
    #[rust]
    doc_tip_shown: bool,
    #[rust]
    fn_external_seen: u64,
    /// The shown layer's script-defined fns: (name, file:line, source).
    vibe_fn_sources: Vec<(String, String, String)>,
    /// An apply just happened: redraw the panel one frame later so the
    /// swatch mirrors the widget AFTER it redrew with the new value.
    swatch_refresh: bool,
    /// The pinned widget's animator tracks, shown as posed state swatches
    /// under the well.
    #[rust]
    state_tracks: Vec<StateTrack>,
    #[rust]
    states_gen: u64,
    #[rust]
    states_uid: u64,
    #[rust]
    states_layer: String,
    #[rust]
    states_paused: bool,
    #[rust]
    states_hover: bool,
    #[rust]
    states_t0: f64,
    #[rust]
    states_frame: NextFrame,
    #[rust]
    states_pause_uid: u64,
    /// SHADER CONSTANTS fold state (open by default: it is the point).
    #[rust(true)]
    tweakables_open: bool,
    /// The Shader tab's row list (constants + inputs of the mirrored layer).
    #[rust]
    shader_list_uid: u64,
    #[rust]
    shader_entries: Vec<VisKind>,
    /// The row whose field was last touched: its doc line rides under it.
    #[rust]
    doc_row: Option<usize>,
    /// The Shader tab's scroll viewport, handed to every well it hosts.
    /// Captured at event time: mid-draw the rect slots answer zero.
    #[rust]
    swatch_clip: Option<Rect>,
    /// The Props list's viewport (props_wrap below the scope control).
    #[rust]
    props_viewport: Option<Rect>,
    /// The theme's colour palette (name, rgba, defined-at), read once.
    #[rust]
    theme_colors: Vec<(String, u32, String)>,
    /// The colour being hover-pulsed app-wide (and when it started).
    #[rust]
    pulse: Option<(u32, f64)>,
    /// Pinned by a remote `op=pulse`: the pointer no longer clears it.
    #[rust]
    pulse_pinned: bool,
    #[rust]
    pulse_ticks: u64,
    #[rust]
    pulse_last_sync: f64,
    /// The theme's definition site (file:line of its first value).
    #[rust]
    theme_site: String,
    #[rust]
    pulse_frame: NextFrame,
    /// A scroll-to-selection servo: each tree draw reports where the
    /// selected row landed and the error is corrected until it is inside
    /// the viewport (estimates and clamping cannot diverge it).
    #[rust]
    tree_scroll_tries: Option<u8>,
    /// Set by the note button; the next event opens the card.
    note_request: bool,
    /// Armed state for the 2.5D exploded z-layer view (M3 wires the
    /// renderer; until then this is the mode flag + visual state).
    #[rust]
    sploded_armed: bool,
    /// Which side-panel tab is active (persists across F12).
    #[rust]
    panel_tab: PanelTab,
    /// The shader tab's draw layer (clicking a material thumbnail switches
    /// the tab here and sets this).
    #[rust]
    vibe_layer: Option<String>,
    /// Tab-bar button uids, captured at draw.
    #[rust]
    tab_uids: [u64; 4],
    /// The two PortalLists' uids (props, tree), captured at ensure.
    #[rust]
    props_list_uid: u64,
    #[rust]
    tree_list_uid: u64,
    /// The flattened widget tree for the tree tab + its generation.
    #[rust]
    tree_rows: Vec<crate::widget_tree::FlatTreeRow>,
    #[rust]
    tree_rows_gen: u64,
    /// Tree rows drawn this frame: (item, target widget uid).
    #[rust]
    tree_visible: Vec<(WidgetRef, u64)>,
    /// Set while the hover outline was driven from the tree tab.
    #[rust]
    tree_hover_active: bool,
    /// The selection uid the tree last auto-scrolled to (scroll once per
    /// selection change; never fight the user's own scrolling).
    #[rust]
    tree_scrolled_uid: u64,
    /// Parent index per tree row (same order as tree_rows).
    #[rust]
    tree_parents: Vec<Option<usize>>,
    /// Child indices per tree row.
    #[rust]
    tree_children: Vec<Vec<usize>>,
    /// Open the readable default levels once per tree refresh.
    #[rust]
    tree_open_defaults_pending: bool,
    /// The prompt TextInput's uid, captured at draw.
    #[rust]
    vibe_prompt_uid: u64,
    /// The Ctrl+Space note card: visible for the current selection.
    #[rust]
    note_open: bool,
    #[rust]
    note_ui: Option<WidgetRef>,
    /// The card's on-screen rect (intercept exemption + grip dragging).
    #[rust]
    note_rect: Option<Rect>,
    /// A grip drag in flight: pointer offset from the card origin.
    #[rust]
    note_drag: Option<Vec2d>,
    #[rust]
    note_text_uid: u64,
    /// Seed the card's TextInput once per open (never clobber typing).
    #[rust]
    note_seed_pending: bool,
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
    /// The body's own margin.right before the panel compressed it.
    #[rust]
    saved_body_right: Option<f64>,
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
        // A dotted `margin.right:` apply fails when the body's margin is a
        // scalar (an f64 has no .right). Read the current legs (scalar
        // margins fan out to all four) and apply one full Inset; remember
        // the body's own right so release restores it, not zero.
        let before = reflect_flat(cx, &body);
        let leg = |name: &str| {
            before
                .iter()
                .find(|(n, _, _)| n == name)
                .and_then(|(_, v, _)| v.parse::<f64>().ok())
        };
        let scalar = leg("margin");
        let left = leg("margin.left").or(scalar).unwrap_or(0.0);
        let top = leg("margin.top").or(scalar).unwrap_or(0.0);
        let bottom = leg("margin.bottom").or(scalar).unwrap_or(0.0);
        if self.saved_body_right.is_none() {
            self.saved_body_right = Some(leg("margin.right").or(scalar).unwrap_or(0.0));
        }
        let right = if desired > 0.5 {
            desired
        } else {
            self.saved_body_right.take().unwrap_or(0.0)
        };
        let chunk = format!(
            "margin: Inset{{left: {left} top: {top} right: {right:.0} bottom: {bottom}}}"
        );
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
        if self.theme_colors.is_empty() {
            self.theme_colors = theme_palette(cx);
            log!("TWEAK theme palette: {} colours", self.theme_colors.len());
        }
        // The shader source view is a plain multiline TextInput, on purpose:
        // the real code editor as a sidebar child would put a CodeView in
        // the main window's widget tree for every app the tweaker rides in.
        // Live-coding needs only text-in/text-out — Ctrl+Enter and the
        // settle timer read `.text()` by path, whatever widget holds it.
        let sidebar = cx.with_vm(|vm| {
            let value = script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*

                // Row templates, hoisted: one source of truth for the Props list,
                // the Shader tab INPUTS list and the shader-constant rows.
                let SectionRowT = FabSection {
                    count := FabLabelSmall { width: Fit margin: Inset{left: 4 top: 1 right: 0 bottom: 0} text: "" }
                }
                let CascadeRowT = View {
                    width: Fill
                    height: Fit
                    flow: Down
                    spacing: 2
                    padding: Inset{left: 8 right: 8 top: 4 bottom: 4}
                    head := View {
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 6
                        align: Align{x: 0.0 y: 0.5}
                        ic_app := View { width: Fit height: Fit visible: false
                            i := Icon { icon_walk: Walk{width: 12 height: Fit} draw_icon +: { color: #xbbbbbb svg: crate_resource("self:resources/icons/icon_file.svg") } }
                        }
                        ic_lib := View { width: Fit height: Fit visible: false
                            i := Icon { icon_walk: Walk{width: 12 height: Fit} draw_icon +: { color: #xbbbbbb svg: crate_resource("self:resources/icons/icon_widget.svg") } }
                        }
                        ic_theme := View { width: Fit height: Fit visible: false
                            i := Icon { icon_walk: Walk{width: 12 height: Fit} draw_icon +: { color: #xbbbbbb svg: crate_resource("self:resources/icons/icon_draw.svg") } }
                        }
                        ic_native := View { width: Fit height: Fit visible: false
                            i := Icon { icon_walk: Walk{width: 12 height: Fit} draw_icon +: { color: #xbbbbbb svg: crate_resource("self:resources/icons/icon_layout.svg") } }
                        }
                        chip := RoundedView {
                            width: Fit height: Fit
                            padding: Inset{left: 5 right: 5 top: 1 bottom: 1}
                            draw_bg +: { color: #x555555 radius: 3. }
                            lbl := FabLabelSmall { width: Fit text: "L0" draw_text +: { color: #x151515 } }
                        }
                        loc := FabLabelDim { width: Fill text: "" max_lines: 1 text_overflow: TextOverflow.Ellipsis }
                    }
                    sets_wrap := View { width: Fill height: Fit visible: false
                        sets := FabLabelSmall { width: Fill margin: Inset{left: 22 top: 0 right: 0 bottom: 0} text: "" }
                    }
                    overridden_wrap := View { width: Fill height: Fit visible: false
                        overridden := FabLabelSmall { width: Fill margin: Inset{left: 22 top: 0 right: 0 bottom: 0} text: "" }
                    }
                }
                let MaterialRowT = View {
                    width: Fill
                    height: 40
                    flow: Right
                    spacing: 6
                    align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 8 right: 6 top: 3 bottom: 3}
                    name := mod.widgets.FabLabelDim {
                        width: 70
                        text: ""
                    }
                    swatch_bg := View {
                        width: Fill
                        height: 30
                        show_bg: true
                        padding: Inset{left: 2 right: 2 top: 2 bottom: 2}
                        draw_bg +: {
                            color: #x606060
                        }
                        swatch := TweakMaterialSwatch {
                            width: Fill
                            height: Fill
                        }
                    }
                }
                let NumRowT = View {
                    width: Fill
                    height: 24
                    flow: Right
                    align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 8 right: 6 top: 0 bottom: 0}
                    spacing: 6
                    name := FabLabelDim {
                        width: Fill
                        text: ""
                        max_lines: 1
                        text_overflow: TextOverflow.Ellipsis
                    }
                    value := FabValueInput {
                        width: 150
                        height: 18
                    }
                    origin := FabLabelSmall { width: 12 margin: Inset{left: 2 top: 2 right: 0 bottom: 0} text: "" }
                }
                let BoolRowT = View {
                    width: Fill
                    height: 24
                    flow: Right
                    align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 8 right: 6 top: 0 bottom: 0}
                    spacing: 6
                    name := FabLabelDim {
                        width: Fill
                        text: ""
                        max_lines: 1
                        text_overflow: TextOverflow.Ellipsis
                    }
                    value := CheckBox {
                        width: Fit
                        height: Fit
                        text: ""
                    }
                    origin := FabLabelSmall { width: 12 margin: Inset{left: 2 top: 2 right: 0 bottom: 0} text: "" }
                }
                let TextRowT = View {
                    width: Fill
                    height: 24
                    flow: Right
                    align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 8 right: 6 top: 0 bottom: 0}
                    spacing: 6
                    name := FabLabelDim {
                        width: Fill
                        text: ""
                        max_lines: 1
                        text_overflow: TextOverflow.Ellipsis
                    }
                    value := TextInput {
                        width: 150
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
                let InfoRowT = FabPropRow {
                    value := FabLabelSmall {
                        width: Fill
                        margin: Inset{left: 0 top: 2 right: 0 bottom: 0}
                        text: ""
                    }
                }
                let SizeRowT = FabPropRow {
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
                let BoxRowT = FabPropRow {
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
                let FlowRowT = FabPropRow {
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
                let AlignRowT = FabPropRow {
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
                let MoreRowT = FabSection {}
                let ColorRowT = View {
                    width: Fill
                    height: 24
                    flow: Right
                    align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 8 right: 6 top: 0 bottom: 0}
                    spacing: 6
                    name := FabLabelDim {
                        width: Fill
                        text: ""
                        max_lines: 1
                        text_overflow: TextOverflow.Ellipsis
                    }
                    value := TextInput {
                        width: 110
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
                    tname_wrap := View { width: Fit height: Fit visible: false
                        tname := Button { width: Fit height: 16 padding: Inset{left: 4 right: 4 top: 1 bottom: 1} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                    }
                    origin := FabLabelSmall { width: 12 margin: Inset{left: 2 top: 2 right: 0 bottom: 0} text: "" }
                }
                let VecRowT = View {
                    width: Fill height: 24 flow: Right align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 8 right: 6 top: 0 bottom: 0} spacing: 4
                    name := FabLabelDim { width: Fill text: "" max_lines: 1 text_overflow: TextOverflow.Ellipsis }
                    vx := FabValueInput { width: 46 height: 18 }
                    vy := FabValueInput { width: 46 height: 18 }
                    vz_wrap := View { width: Fit height: Fit visible: false
                        vz := FabValueInput { width: 46 height: 18 }
                    }
                    vw_wrap := View { width: Fit height: Fit visible: false
                        vw := FabValueInput { width: 46 height: 18 }
                    }
                }
                let InsetRowT = View {
                    width: Fill height: 24 flow: Right align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 8 right: 6 top: 0 bottom: 0} spacing: 3
                    name := FabLabelDim { width: Fill text: "" max_lines: 1 text_overflow: TextOverflow.Ellipsis }
                    il := FabValueInput { width: 40 height: 18 }
                    it := FabValueInput { width: 40 height: 18 }
                    ir := FabValueInput { width: 40 height: 18 }
                    ib := FabValueInput { width: 40 height: 18 }
                }
                let MetricsRowT = View {
                    width: Fill height: 24 flow: Right align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 8 right: 6 top: 0 bottom: 0} spacing: 4
                    name := FabLabelDim { width: Fill text: "" max_lines: 1 text_overflow: TextOverflow.Ellipsis }
                    m0 := FabValueInput { width: 44 height: 18 }
                    m1 := FabValueInput { width: 44 height: 18 }
                    m2 := FabValueInput { width: 44 height: 18 }
                }
                let SizeFieldRowT = View {
                    width: Fill height: 24 flow: Right align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 8 right: 6 top: 0 bottom: 0} spacing: 4
                    name := FabLabelDim { width: Fill text: "" max_lines: 1 text_overflow: TextOverflow.Ellipsis }
                    sf_fill := Button { width: Fit height: 16 padding: Inset{left: 5 right: 5 top: 1 bottom: 1} text: "Fill" draw_text +: { text_style +: { font_size: 7.0 } } }
                    sf_fit := Button { width: Fit height: 16 padding: Inset{left: 5 right: 5 top: 1 bottom: 1} text: "Fit" draw_text +: { text_style +: { font_size: 7.0 } } }
                    sf_num := FabValueInput { width: 56 height: 18 }
                }
                let NoEditorRowT = View {
                    width: Fill height: 24 flow: Right align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 8 right: 6 top: 0 bottom: 0} spacing: 6
                    name := FabLabelDim { width: Fill text: "" max_lines: 1 text_overflow: TextOverflow.Ellipsis }
                    ne := FabLabelSmall { width: Fit text: "no editor yet" }
                }
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
                    filter_row := View {
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 4
                        align: Align{x: 0.0 y: 0.5}
                        search := FabSearch {}
                        sploded := Button {
                            width: 28
                            height: 24
                            padding: Inset{left: 5 right: 5 top: 3 bottom: 3}
                            margin: Inset{left: 0 right: 4 top: 0 bottom: 0}
                            text: ""
                            icon_walk: Walk{width: 15 height: Fit}
                            draw_icon +: {
                                color: #xd8d8d8
                                svg: crate_resource("self:resources/icons/sploded.svg")
                            }
                        }
                        note := Button {
                            width: Fit
                            height: 24
                            padding: Inset{left: 6 right: 6 top: 3 bottom: 3}
                            margin: Inset{left: 0 right: 4 top: 0 bottom: 0}
                            text: "note"
                        }
                        spread_wrap := View {
                            width: Fit
                            height: Fit
                            visible: false
                            spread := FabValueInput {
                                width: 44
                                height: 18
                                margin: Inset{left: 0 right: 4 top: 0 bottom: 0}
                            }
                        }
                    }
                    tab_row := View {
                        width: Fill
                        height: 22
                        flow: Right
                        spacing: 2
                        padding: Inset{left: 4 right: 4 top: 0 bottom: 0}
                        tab_props := Button { width: Fit height: 20 padding: Inset{left: 8 right: 8 top: 2 bottom: 2} text: "Props" draw_text +: { text_style +: { font_size: 8.0 } } }
                        tab_shader := Button { width: Fit height: 20 padding: Inset{left: 8 right: 8 top: 2 bottom: 2} text: "Shader" draw_text +: { text_style +: { font_size: 8.0 } } }
                        tab_tree := Button { width: Fit height: 20 padding: Inset{left: 8 right: 8 top: 2 bottom: 2} text: "Tree" draw_text +: { text_style +: { font_size: 8.0 } } }
                        tab_theme := Button { width: Fit height: 20 padding: Inset{left: 8 right: 8 top: 2 bottom: 2} text: "Theme" draw_text +: { text_style +: { font_size: 8.0 } } }
                    }
                    shader_col := ScrollYView {
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 6
                        padding: Inset{left: 8 right: 8 top: 6 bottom: 6}
                        shader_title := FabHeaderLabel {
                            width: Fill
                            text: ""
                        }
                        shader_doc := FabLabelSmall {
                            width: Fill
                            text: ""
                            max_lines: 1
                            text_overflow: TextOverflow.Ellipsis
                        }
                        big := TweakMaterialSwatch {
                            width: Fill
                            height: 150
                        }
                        states_row := View {
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 8
                            visible: false
                            states_pause := Button { width: Fit height: 18 padding: Inset{left: 6 right: 6 top: 1 bottom: 1} text: "pause" draw_text +: { text_style +: { font_size: 8.0 } } }
                            st0 := View { width: Fill height: Fit flow: Down spacing: 2 visible: false
                                lbl := FabLabelSmall { text: "" }
                                sw := TweakMaterialSwatch { width: Fill height: 150 }
                            }
                            st1 := View { width: Fill height: Fit flow: Down spacing: 2 visible: false
                                lbl := FabLabelSmall { text: "" }
                                sw := TweakMaterialSwatch { width: Fill height: 150 }
                            }
                            st2 := View { width: Fill height: Fit flow: Down spacing: 2 visible: false
                                lbl := FabLabelSmall { text: "" }
                                sw := TweakMaterialSwatch { width: Fill height: 150 }
                            }
                            st3 := View { width: Fill height: Fit flow: Down spacing: 2 visible: false
                                lbl := FabLabelSmall { text: "" }
                                sw := TweakMaterialSwatch { width: Fill height: 150 }
                            }
                            st4 := View { width: Fill height: Fit flow: Down spacing: 2 visible: false
                                lbl := FabLabelSmall { text: "" }
                                sw := TweakMaterialSwatch { width: Fill height: 150 }
                            }
                            st5 := View { width: Fill height: Fit flow: Down spacing: 2 visible: false
                                lbl := FabLabelSmall { text: "" }
                                sw := TweakMaterialSwatch { width: Fill height: 150 }
                            }
                        }
                        shader_rows_wrap := View {
                            width: Fill
                            height: Fit
                            visible: false
                        shader_rows := PortalList {
                            width: Fill
                            height: 320
                            margin: Inset{left: 0 top: 2 right: 0 bottom: 0}
                            drag_scrolling: false
                            SectionRow := SectionRowT {}
                            MaterialRow := MaterialRowT {}
                            NumRow := NumRowT {}
                            BoolRow := BoolRowT {}
                            TextRow := TextRowT {}
                            InfoRow := InfoRowT {}
                            SizeRow := SizeRowT {}
                            BoxRow := BoxRowT {}
                            FlowRow := FlowRowT {}
                            AlignRow := AlignRowT {}
                            MoreRow := MoreRowT {}
                            ColorRow := ColorRowT {}
                            VecRow := VecRowT {}
                            InsetRow := InsetRowT {}
                            MetricsRow := MetricsRowT {}
                            SizeFieldRow := SizeFieldRowT {}
                            NoEditorRow := NoEditorRowT {}
                        }
                        }
                        src_fold := Button {
                            width: Fit
                            height: 20
                            padding: Inset{left: 8 right: 8 top: 2 bottom: 2}
                            text: "+ source"
                            draw_text +: { text_style +: { font_size: 8.0 } }
                        }
                        src_scroll := View {
                            width: Fill
                            height: Fit
                            show_bg: true
                            draw_bg +: { color: #x1b1b1b }
                            padding: Inset{left: 6 right: 6 top: 4 bottom: 4}
                            shader_src := TextInput {
                                width: Fill
                                height: Fit
                                is_multiline: true
                                draw_bg +: { color: #x1b1b1b }
                                draw_text +: {
                                    color: #xd0d0d0
                                    text_style: theme.font_code
                                    text_style +: { font_size: 7.5 }
                                }
                            }
                        }
                        prompt := TextInput {
                            width: Fill
                            height: 64
                            is_multiline: true
                            empty_text: "what should this shader's CODE do differently\u{2026} Ctrl+Enter sends"
                            draw_bg +: {
                                color: #x1b1b1b
                                border_radius: 3.0
                            }
                            draw_text +: {
                                color: #xe6e6e6
                                text_style +: { font_size: 8.5 }
                            }
                        }
                        vibe_status := FabLabelSmall {
                            width: Fill
                            text: ""
                            draw_text +: { color: #xffa040 }
                        }
                        vibe_hint := FabLabelSmall {
                            width: Fill
                            text: "Ctrl+Enter sends \u{00b7} the agent rewrites only the fn code \u{00b7} colours and sizes stay in Props"
                        }
                        doc_tip := Tooltip {
                            width: 0
                            height: 0
                            clip_x: false
                            clip_y: false
                            content := RoundedView {
                                width: Fit
                                height: Fit
                                padding: Inset{left: 8 right: 8 top: 6 bottom: 6}
                                draw_bg +: {
                                    color: #x2a2a2a
                                    border_size: 1.0
                                    border_color: #x555555
                                    radius: 3.
                                }
                                tooltip_label := FabLabelSmall {
                                    width: 220
                                    text: ""
                                }
                            }
                        }
                    }
                    tree_wrap := View {
                        width: Fill
                        height: Fill
                        flow: Down
                        tree := FileTree {}
                    }
                    props_wrap := View {
                        width: Fill
                        height: Fill
                        flow: Down
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
                        SectionRow := SectionRowT {}
                        MaterialRow := MaterialRowT {}
                        NumRow := NumRowT {}
                        BoolRow := BoolRowT {}
                        TextRow := TextRowT {}
                        InfoRow := InfoRowT {}
                        CascadeRow := CascadeRowT {}
                        SizeRow := SizeRowT {}
                        BoxRow := BoxRowT {}
                        FlowRow := FlowRowT {}
                        AlignRow := AlignRowT {}
                        MoreRow := MoreRowT {}
                        ColorRow := ColorRowT {}
                        VecRow := VecRowT {}
                        InsetRow := InsetRowT {}
                        MetricsRow := MetricsRowT {}
                        SizeFieldRow := SizeFieldRowT {}
                        NoEditorRow := NoEditorRowT {}
                    }
                    }
                    ident_footer := View {
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 2
                        show_bg: true
                        draw_bg +: { color: #x2b2b30 }
                        divider := View { width: Fill height: 1 margin: Inset{left: 0 top: 0 right: 0 bottom: 4} show_bg: true draw_bg +: { color: #x4a4a52 } }
                        padding: Inset{left: 8 right: 8 top: 0 bottom: 6}
                        scope_row := View {
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 3
                            padding: Inset{left: 0 right: 0 top: 0 bottom: 2}
                            scope_line := View {
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 4
                                align: Align{x: 0.0 y: 0.5}
                                scope_label := FabLabelSmall { width: Fit text: "scope" }
                                scope_this := Button { width: Fit height: 18 padding: Inset{left: 8 right: 8 top: 1 bottom: 1} text: "this" draw_text +: { text_style +: { font_size: 8.0 } } }
                                scope_all := Button { width: Fit height: 18 padding: Inset{left: 8 right: 8 top: 1 bottom: 1} text: "all" draw_text +: { text_style +: { font_size: 8.0 } } }
                            }
                            scope_doc := FabLabelSmall { width: Fill text: "" max_lines: 1 text_overflow: TextOverflow.Ellipsis }
                            scope_origin := FabLabelSmall { width: Fill text: "" }
                        }
                        title_label := FabLabelDim { width: Fill text: "tweak" max_lines: 1 text_overflow: TextOverflow.Ellipsis }
                        path_label := FabLabelSmall { width: Fill text: "click a widget to inspect it" max_lines: 1 text_overflow: TextOverflow.Ellipsis }
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        // Make the sidebar part of the widget tree (under this tweaker):
        // /snap lists its rows, so the same remote agent that watches the
        // session can drive the sidebar's fields too.
        cx.widget_tree_insert_child(self.uid, live_id!(sidebar), sidebar.clone());
        self.props_list_uid = sidebar
            .child(live_id!(props_wrap))
            .child(live_id!(props))
            .widget_uid()
            .0;
        self.tree_list_uid = sidebar
            .child(live_id!(tree_wrap))
            .child(live_id!(tree))
            .widget_uid()
            .0;
        self.shader_list_uid = sidebar
            .child(live_id!(shader_col))
            .child(live_id!(shader_rows))
            .widget_uid()
            .0;
        self.sidebar = Some(sidebar);
    }

    fn ensure_note_ui(&mut self, cx: &mut Cx) {
        if self.note_ui.is_some() {
            return;
        }
        let ui = cx.with_vm(|vm| {
            let value = script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View {
                    width: Fill
                    height: 72
                    flow: Down
                    show_bg: true
                    draw_bg +: {
                        color: #x2d2d36
                    }
                    grip := View {
                        width: Fill
                        height: 11
                        show_bg: true
                        draw_bg +: {
                            color: #x444452
                        }
                    }
                    note_text := TextInput {
                        width: Fill
                        height: 54
                        empty_text: "note on this item \u{2014} Insert or the note button: pinned, else hovered \u{00b7} Esc closes"
                        draw_bg +: {
                            color: #x22222a
                        }
                        draw_text +: {
                            color: #xe8e8d0
                            text_style +: { font_size: 8.5 }
                        }
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        cx.widget_tree_insert_child(self.uid, live_id!(note), ui.clone());
        self.note_ui = Some(ui);
    }

    /// Rebuild the row bindings from the selection's reflected properties:
    /// classify into sections, order each section (box-model progression for
    /// layout, surface-then-content with colors leading for style), and mark
    /// what differs from its session-original (resettable).
    /// The Theme tab's rows: every theme colour (Colours), number
    /// (Spacing & sizes) and font size, each editable in place.
    fn rebuild_theme_rows(&mut self, cx: &mut Cx) {
        self.rows.clear();
        self.cascade.clear();
        self.doc_row = None;
        self.radius_prop = None;
        let (diff, overrides): (Vec<TweakDiffEntry>, Vec<String>) = {
            let s = session().lock().unwrap();
            (
                s.diff.iter().filter(|e| e.scope == "theme").cloned().collect(),
                s.theme_overrides.iter().map(|(n, _)| n.clone()).collect(),
            )
        };
        let mut site = String::new();
        let mut values = theme_values(cx);
        values.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, _, value, loc) in values {
            if site.is_empty() {
                site = loc.clone();
            }
            let (kind, text, section) = match value {
                ThemeVal::Color(c) => (RowKind::Color, hex_of(c), SectionKind::Style),
                ThemeVal::Num(f) => (
                    RowKind::Num,
                    fmt_f64(f),
                    if name.starts_with("font") { SectionKind::Text } else { SectionKind::Layout },
                ),
            };
            let original = diff.iter().find(|e| e.prop == name).map(|e| e.old.clone());
            let changed = overrides.iter().any(|n| *n == name) || original.as_ref().is_some_and(|o| *o != text);
            self.row_docs.insert(name.clone(), format!("theme value, defined in {loc}"));
            self.rows.push(RowBinding {
                prop: name,
                kind,
                value: text,
                quoted: false,
                section,
                set: true,
                changed,
                original,
                field_uid: 0,
                swatch_uid: 0,
                struct_kind: StructKind::None,
                comp_vals: Vec::new(),
                comp_uids: Vec::new(),
                mode_uids: Vec::new(),
                alt_uids: Vec::new(),
                const_ref: None,
                theme_match: None,
                theme_uid: 0,
            });
        }
        self.theme_site = site;
        self.rows_uid = THEME_ROWS;
    }

    /// A `name: value` chunk from a theme row's editor.
    fn theme_apply_chunk(&mut self, cx: &mut Cx, chunk: &str) {
        if let Some((prop, value)) = single_prop_chunk(chunk) {
            if let Err(error) = self.theme_set(cx, &prop, value.trim(), "sidebar") {
                log!("TWEAK theme apply failed: {error}");
            }
        }
    }

    /// Double-click reset: the session-original value back.
    fn theme_reset(&mut self, cx: &mut Cx, name: &str) {
        let original = session()
            .lock()
            .unwrap()
            .diff
            .iter()
            .find(|e| e.scope == "theme" && e.prop == name)
            .map(|e| e.old.clone());
        if let Some(original) = original {
            if let Err(error) = self.theme_set(cx, name, &original, "reset") {
                log!("TWEAK theme reset failed: {error}");
            }
        }
    }

    /// Set one global theme value. A colour: every draw buffer slot
    /// holding it is retargeted live, app-wide, through the pulse's
    /// identity ledger (kept in sync after each draw), and the theme
    /// object in the script heap follows, so widgets applied from now on
    /// bake the new colour too. A number: the heap value (see below).
    /// Ledgered at the theme's own definition site with scope "theme";
    /// undoable.
    fn theme_set(&mut self, cx: &mut Cx, name: &str, text: &str, origin: &str) -> Result<(), String> {
        let values = theme_values(cx);
        let (key, value, loc) = values
            .iter()
            .find(|(n, _, _, _)| n == name)
            .map(|(_, k, v, l)| (*k, *v, l.clone()))
            .ok_or_else(|| format!("no theme value {name:?}"))?;
        // `theme.color_y` as a value: that colour's current hex.
        let text = match text.strip_prefix("theme.") {
            Some(other) => match values.iter().find(|(n, _, _, _)| n == other).map(|(_, _, v, _)| *v) {
                Some(ThemeVal::Color(c)) => hex_of(c),
                Some(ThemeVal::Num(f)) => fmt_f64(f),
                None => return Err(format!("no theme value {other:?}")),
            },
            None => text.to_string(),
        };
        let old_text = match value {
            ThemeVal::Color(c) => hex_of(c),
            ThemeVal::Num(f) => fmt_f64(f),
        };
        let overridden = session().lock().unwrap().theme_overrides.iter().any(|(n, _)| n == name);
        if old_text == text && !overridden {
            return Ok(());
        }
        match value {
            ThemeVal::Color(current) => {
                let (rgba, _) = parse_hex(&text).ok_or_else(|| format!("{text:?} is not a colour"))?;
                let new = packed_of(rgba);
                // The theme module is immutable to scripts; a design tool
                // edits the value in place, at the level that defines it.
                theme_heap_set(cx, key, ScriptValue::from_color(new));
                let mut s = session().lock().unwrap();
                match s.theme_overrides.iter().position(|(n, _)| n == name) {
                    Some(i) => {
                        if new == packed_of(s.theme_overrides[i].1.target) {
                            // Back at the original: restore and forget.
                            let (_, st) = s.theme_overrides.remove(i);
                            drop(s);
                            pulse_restore(cx, &st);
                        } else {
                            s.theme_overrides[i].1.fixed = Some(rgba);
                            drop(s);
                        }
                    }
                    None => {
                        let mut st = PulseState::new(current);
                        st.fixed = Some(rgba);
                        s.theme_overrides.push((name.to_string(), st));
                        drop(s);
                    }
                }
                theme_overrides_sync(cx);
                hook_sync(cx);
                let overrides = std::mem::take(&mut session().lock().unwrap().theme_overrides);
                for (_, st) in &overrides {
                    pulse_repaint(cx, st);
                }
                session().lock().unwrap().theme_overrides = overrides;
                self.theme_colors = theme_palette(cx);
            }
            ThemeVal::Num(_) => {
                let f: f64 = text.parse().map_err(|_| format!("{text:?} is not a number"))?;
                // Numbers are baked into layouts at apply time: the heap
                // holds the new value for everything applied from now on
                // and the ledger carries it to the source; existing layout
                // re-flows when the edit lands (a live reload cannot
                // redefine the immutable widget modules today).
                theme_heap_set(cx, key, ScriptValue::from_f64(f));
            }
        }
        let now = cx.seconds_since_app_start();
        let mut s = session().lock().unwrap();
        s.suppress_until = now + SUPPRESS_LINGER;
        s.apply_gen += 1;
        s.next_seq += 1;
        let entry = TweakDiffEntry {
            seq: s.next_seq,
            path: "theme".to_string(),
            prop: name.to_string(),
            old: old_text,
            new: text.clone(),
            origin: loc,
            siblings: 0,
            scope: "theme".to_string(),
        };
        if origin != "undo" && origin != "redo" {
            log!("TWEAK {} theme {} {} -> {} ({})", origin, entry.prop, entry.old, entry.new, entry.origin);
            track_undo(&mut s, &entry);
        }
        s.diff.push(entry);
        drop(s);
        if let Some(row) = self.rows.iter_mut().find(|r| r.prop == name) {
            row.value = text;
            row.changed = true;
        }
        cx.redraw_all();
        Ok(())
    }

    fn rebuild_rows(&mut self, cx: &mut Cx, sel_uid: u64, sel_path: &str) {
        let widget = cx.widget_tree().widget(WidgetUid(sel_uid));
        if widget.is_empty() {
            self.rows.clear();
            self.rows_uid = 0;
            self.radius_prop = None;
            return;
        }
        self.rows.clear();
        self.doc_row = None;
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
            let (struct_kind, comp_vals) = parse_struct(&display);
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
                struct_kind,
                comp_vals,
                comp_uids: Vec::new(),
                mode_uids: Vec::new(),
                alt_uids: Vec::new(),
                const_ref: None,
                theme_match: None,
                theme_uid: 0,
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
        // The selection's draw layers, in row order — the material cards.
        self.materials.clear();
        for row in &self.rows {
            if row.section == SectionKind::Style {
                let first = row.prop.split('.').next().unwrap_or("");
                if first.starts_with("draw_") && !self.materials.iter().any(|m| m == first) {
                    self.materials.push(first.to_string());
                }
            }
        }
        // The CASCADE section: read-only rows, one block per construction
        // level, pushed after the sort so they keep exactly this order.
        {
            let widget = cx.widget_tree().widget(WidgetUid(sel_uid));
            self.row_docs = collect_row_docs(cx, &widget);
            // SHADER CONSTANTS: the annotated literals inside each draw
            // layer's fn bodies — actual values IN shader code, listed
            // per layer with the annotation as their doc.
            let materials = self.materials.clone();
            for (mi, layer) in materials.iter().enumerate() {
                let mut seen: Vec<String> = Vec::new();
                for (_, _, name, doc, initial, value, loc) in layer_consts(cx, &widget, layer, mi == 0) {
                    // One knob per name: a literal annotated at two sites
                    // in the same shader is one constant (both patch).
                    if seen.contains(&name) {
                        continue;
                    }
                    seen.push(name.clone());
                    let prop = if self.rows.iter().any(|r| r.prop == name) {
                        format!("{layer} \u{00b7} {name}")
                    } else {
                        name.clone()
                    };
                    // The tooltip names the literal's site: these live in shader code.
                    self.row_docs.insert(prop.clone(), if doc.is_empty() { loc } else { format!("{doc}\n{loc}") });
                    self.rows.push(RowBinding {
                        prop,
                        kind: RowKind::Num,
                        value: fmt_f64(value as f64),
                        quoted: false,
                        section: SectionKind::Style,
                        set: true,
                        changed: value != initial,
                        original: Some(fmt_f64(initial as f64)),
                        field_uid: 0,
                        swatch_uid: 0,
                        struct_kind: StructKind::None,
                        comp_vals: Vec::new(),
                        comp_uids: Vec::new(),
                        mode_uids: Vec::new(),
                        alt_uids: Vec::new(),
                        const_ref: Some(ConstRef { layer: layer.clone(), name, initial }),
                        theme_match: None,
                        theme_uid: 0,
                    });
                }
            }
            self.origin_levels.clear();
            let levels = cascade_levels(cx, &widget);
            self.cascade_level_count = levels.len();
            for (i, lvl) in levels.iter().enumerate() {
                for (key, overridden) in &lvl.sets {
                    if !overridden {
                        self.origin_levels.entry(key.clone()).or_insert(i);
                    }
                }
            }
            // The CASCADE section renders these directly (one row per
            // level: icon, chip, file:line, what it sets); the rows list
            // carries no Info rows for it any more.
            self.cascade = levels;
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

    /// A row matches the filter on its name, its value text, or its
    /// `/** */` annotation (docs are searchable: "banding" finds
    /// color_dither through its doc line).
    fn row_matches_filter(&self, row: &RowBinding) -> bool {
        self.filter.is_empty()
            || row.prop.to_lowercase().contains(&self.filter)
            || row.value.to_lowercase().contains(&self.filter)
            || self
                .row_docs
                .get(&row.prop)
                .is_some_and(|doc| doc.to_lowercase().contains(&self.filter))
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
        // SHADER CONSTANTS: the annotated literals inside the draw layers'
        // fn bodies — "actual values IN shader code" — each with its doc
        // line. Absent when the widget's shaders carry none. (Annotated
        // props are not constants: they stay in their sections with the
        // gold marker and the doc line under the touched row.)
        if !filtering {
            let tweak: Vec<usize> = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| r.const_ref.is_some())
                .map(|(i, _)| i)
                .collect();
            if !tweak.is_empty() {
                out.push(VisKind::TweakHeader(tweak.len(), self.tweakables_open));
                if self.tweakables_open {
                    for index in tweak {
                        out.push(VisKind::Tweakable(index));
                    }
                }
            }
        }
        for section in SECTION_ORDER {
            if section == SectionKind::Cascade {
                if filtering || self.cascade.is_empty() {
                    continue;
                }
                let open = !self.collapsed[section.index()];
                out.push(VisKind::Section(section, self.cascade.len(), open));
                if open {
                    for i in 0..self.cascade.len() {
                        out.push(VisKind::CascadeLevel(i));
                    }
                }
                continue;
            }
            let members: Vec<usize> = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, row)| row.section == section && row.const_ref.is_none() && self.row_matches_filter(row))
                .map(|(index, _)| index)
                .collect();
            let theme = self.panel_tab == PanelTab::Theme;
            let composites: Vec<VisKind> = if section == SectionKind::Layout && !filtering && !theme {
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
            // A section with no primary row read as an empty header over a
            // "show all": lead with its first three rows instead.
            let primary = members
                .iter()
                .filter(|&&i| match section {
                    SectionKind::Style => theme || Self::style_curated(&self.rows[i]),
                    SectionKind::Layout => theme || !Self::layout_composited(&self.rows[i].prop),
                    _ => true,
                })
                .count();
            let force_show = if !filtering && !expanded && primary == 0 { 3usize } else { 0 };
            let mut forced = 0usize;
            let mut hidden = 0usize;
            let mut last_material: Option<String> = None;
            for index in members {
                let row = &self.rows[index];
                // Theme rows are all primary: nothing is folded behind a
                // "show all".
                let in_tail = match section {
                    _ if theme => false,
                    SectionKind::Layout => {
                        !filtering && Self::layout_composited(&row.prop)
                            || (!expanded && !Self::layout_composited(&row.prop))
                    }
                    SectionKind::Style => !expanded && !Self::style_curated(row),
                    _ => false,
                };
                if !filtering && in_tail && forced < force_show {
                    forced += 1;
                } else if !filtering && in_tail {
                    // Rows folded into composites never re-appear; the
                    // rest count toward the expander.
                    if !(section == SectionKind::Layout && Self::layout_composited(&row.prop))
                        && !(section == SectionKind::Style && Self::style_curated(row))
                    {
                        hidden += 1;
                    }
                    continue;
                }
                // Material card header before each draw layer's first row.
                if section == SectionKind::Style && !filtering {
                    let first = row.prop.split('.').next().unwrap_or("");
                    if first.starts_with("draw_") && last_material.as_deref() != Some(first) {
                        if let Some(mi) = self.materials.iter().position(|m| m == first) {
                            out.push(VisKind::Material(mi));
                        }
                        last_material = Some(first.to_string());
                    }
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
        // The panel is flat chrome over the exploded view: pointer events
        // inside the band flow through in plain window coordinates.
        cx.sploded_set_flat_band(Some(band));

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

        if self.panel_tab == PanelTab::Theme {
            let colours = self.rows.iter().filter(|r| r.kind == RowKind::Color).count();
            let footer = sidebar.child(live_id!(ident_footer));
            footer
                .child(live_id!(title_label))
                .set_text(cx, &format!("Theme  \u{2022}  {colours} colours  \u{2022}  {} values", self.rows.len() - colours));
            let site = self.theme_site.split(':').next().unwrap_or("").to_string();
            footer.child(live_id!(path_label)).set_text(cx, &format!("edits land in {site}"));
            footer.child(live_id!(scope_row)).set_visible(cx, false);
        } else {
            sidebar.child(live_id!(ident_footer)).child(live_id!(scope_row)).set_visible(cx, sel.is_some());
        }
        match sel {
            _ if self.panel_tab == PanelTab::Theme => {}
            Some(sel) => {
                let head = {
                    let mut head = format!("{}  \u{2022}  {} props", sel.ty, self.rows.len());
                    // Depth readout: which plane the selection sits on in
                    // the exploded view ("is it stacked deep, or is the
                    // z-step just insane?").
                    if cx.sploded_active() {
                        if let Some(level) = cx.sploded_depth_of(sel.uid) {
                            head.push_str(&format!("  \u{2022}  L{level}/{}", cx.sploded_max_level()));
                        }
                    }
                    head
                };
                sidebar
                    .child(live_id!(ident_footer)).child(live_id!(title_label))
                    .set_text(cx, &head);
                let shown_path = tail_ellipsis(&display_path(cx, sel.uid), 48);
                sidebar
                    .child(live_id!(ident_footer))
                    .child(live_id!(path_label))
                    .set_text(cx, &shown_path);
                {
                    let all = session().lock().unwrap().scope_all;
                    let row = sidebar.child(live_id!(ident_footer)).child(live_id!(scope_row));
                    row.child(live_id!(scope_line)).child(live_id!(scope_this)).set_text(cx, "this");
                    row.child(live_id!(scope_line)).child(live_id!(scope_all)).set_text(cx, &format!("all {}s", sel.ty));
                    set_button_fill(cx, row.child(live_id!(scope_line)).child(live_id!(scope_this)), !all);
                    set_button_fill(cx, row.child(live_id!(scope_line)).child(live_id!(scope_all)), all);
                    row.child(live_id!(scope_doc)).set_text(
                        cx,
                        &format!("this: only this instance \u{00b7} all {}s: every {} in the app (edits the type's definition)", sel.ty, sel.ty),
                    );
                    let widget = cx.widget_tree().widget(WidgetUid(sel.uid));
                    let origin = if widget.is_empty() {
                        String::new()
                    } else if all {
                        type_origin(cx, &widget)
                    } else {
                        source_origin(cx, &widget)
                    };
                    let base = origin.rsplit('/').next().unwrap_or(&origin).to_string();
                    row.child(live_id!(scope_origin)).set_text(cx, &if base.is_empty() { String::new() } else { format!("edits land in {base}") });
                }
            }
            None => {
                sidebar.child(live_id!(ident_footer)).child(live_id!(title_label)).set_text(cx, "tweak");
                sidebar
                    .child(live_id!(ident_footer)).child(live_id!(path_label))
                    .set_text(cx, "click a widget to inspect it");
            }
        }
        self.search_uid = sidebar
            .child(live_id!(filter_row))
            .child(live_id!(search))
            .child(live_id!(input))
            .widget_uid()
            .0;
        self.sploded_uid = sidebar
            .child(live_id!(filter_row))
            .child(live_id!(sploded))
            .widget_uid()
            .0;
        self.note_uid = sidebar
            .child(live_id!(filter_row))
            .child(live_id!(note))
            .widget_uid()
            .0;
        self.scope_this_uid = sidebar.child(live_id!(ident_footer)).child(live_id!(scope_row)).child(live_id!(scope_line)).child(live_id!(scope_this)).widget_uid().0;
        self.scope_all_uid = sidebar.child(live_id!(ident_footer)).child(live_id!(scope_row)).child(live_id!(scope_line)).child(live_id!(scope_all)).widget_uid().0;
        let spread_wrap = sidebar.child(live_id!(filter_row)).child(live_id!(spread_wrap));
        let spread = spread_wrap.child(live_id!(spread));
        self.spread_uid = spread.widget_uid().0;
        if let Some(mut field) = spread.borrow_mut::<FabValueInput>() {
            field.set_hint(
                Some(SPLODED_SPREAD_MIN as f64),
                Some(SPLODED_SPREAD_MAX as f64),
                Some(0.01),
            );
            let spread_now = cx.sploded_spread() as f64;
            field.set_value(cx, spread_now);
        }
        let spread_on = cx.sploded_will_be_active();
        spread_wrap.set_visible(cx, spread_on);
        if self.focus_search_pending {
            let input = sidebar
                .child(live_id!(filter_row))
                .child(live_id!(search))
                .child(live_id!(input));
            if input.area() != Area::Empty {
                cx.set_key_focus(input.area());
                self.focus_search_pending = false;
            }
        }
        // The panel tabs: Props / Shader / Tree, one content visible.
        {
            let tab = self.panel_tab;
            sidebar
                .child(live_id!(props_wrap))
                .set_visible(cx, matches!(tab, PanelTab::Props | PanelTab::Theme));
            sidebar
                .child(live_id!(shader_col))
                .set_visible(cx, tab == PanelTab::Shader);
            sidebar
                .child(live_id!(tree_wrap))
                .set_visible(cx, tab == PanelTab::Tree);
            let tab_row = sidebar.child(live_id!(tab_row));
            let tabs = [
                (live_id!(tab_props), PanelTab::Props, "Props"),
                (live_id!(tab_shader), PanelTab::Shader, "Shader"),
                (live_id!(tab_tree), PanelTab::Tree, "Tree"),
                (live_id!(tab_theme), PanelTab::Theme, "Theme"),
            ];
            for (i, (id, t, label)) in tabs.into_iter().enumerate() {
                let btn = tab_row.child(id);
                btn.set_text(cx, label);
                set_button_fill(cx, btn.clone(), t == tab);
                self.tab_uids[i] = btn.widget_uid().0;
            }
            // Shader tab content: the layer's live preview + doc + prompt.
            if tab == PanelTab::Shader {
                // Default to the selection's first draw layer (not every
                // widget has a draw_bg).
                let layer = self
                    .vibe_layer
                    .clone()
                    .or_else(|| self.materials.first().cloned())
                    .unwrap_or_else(|| "draw_bg".to_string());
                let col = sidebar.child(live_id!(shader_col));
                {
                    let status = session().lock().unwrap().vibe_status.clone();
                    col.child(live_id!(vibe_status)).set_text(cx, &status);
                }
                // The layer the Shader tab shows is the one a prompt or an
                // editor apply targets.
                self.vibe_layer = Some(layer.clone());
                col.child(live_id!(shader_title)).set_text(cx, &layer);
                let doc = self.row_docs.get(&layer).cloned().unwrap_or_default();
                // One line: the label ellipsises at its own width (max_lines
                // 1); the whole doc rides on hover as a tooltip.
                let doc_line = doc.lines().next().unwrap_or("").trim().to_string();
                col.child(live_id!(shader_doc)).set_text(cx, &doc_line);
                // Flowing text: the source comment's hard breaks are not
                // paragraph breaks.
                let doc_full = doc.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
                self.shader_doc_full = if doc_full.chars().count() > 40 || doc.trim().lines().count() > 1 {
                    doc_full
                } else {
                    String::new()
                };
                self.shader_src_uid = col
                    .child(live_id!(src_scroll))
                    .child(live_id!(shader_src))
                    .widget_uid()
                    .0;
                self.vibe_prompt_uid = col.child(live_id!(prompt)).widget_uid().0;
                // The source editor unfolds on demand only; folded, the tab
                // is the swatch + doc + prompt.
                let fold = col.child(live_id!(src_fold));
                self.shader_fold_uid = fold.widget_uid().0;
                fold.set_text(
                    cx,
                    if self.shader_src_open { "- source" } else { "+ source" },
                );
                col.child(live_id!(src_scroll)).set_visible(cx, self.shader_src_open);
                // The shader as written: pixel (and vertex when the layer
                // sets its own) with their docs — what the prompt rewrites.
                {
                    let widget = cx.widget_tree().widget(WidgetUid(self.rows_uid));
                    let fns = if widget.is_empty() {
                        Vec::new()
                    } else {
                        layer_fn_sources(cx, &widget, &layer)
                    };
                    let mut text = String::new();
                    let overrides = session().lock().unwrap().fn_overrides.clone();
                    for (name, loc, src) in &fns {
                        if !text.is_empty() {
                            text.push_str("\n\n");
                        }
                        // A fn applied live from this view shows as applied,
                        // not as the file has it.
                        let key = (self.rows_uid, layer.clone(), name.clone());
                        let body = overrides.get(&key).cloned().unwrap_or_else(|| reindent_two(src));
                        text.push_str(&format!("// {name} \u{2014} {loc}\n{body}"));
                    }
                    if text.is_empty() {
                        text = "(no script-defined pixel/vertex fn on this layer \u{2014} the type's own shader)".to_string();
                    }
                    self.vibe_fn_sources = fns;
                    let editor = col.child(live_id!(src_scroll)).child(live_id!(shader_src));
                    // While a person types here, the view is the source of
                    // truth: same widget+layer, no outside apply since, and
                    // the editor focused or holding exactly what it last
                    // applied (a failed edit stays on screen to be fixed).
                    let key = (self.rows_uid, layer.clone());
                    let external = session().lock().unwrap().fn_override_gen;
                    let typing = self.live_key == key
                        && self.fn_external_seen == external
                        && (cx.has_key_focus(editor.area()) || editor.text() == self.live_last_applied);
                    if editor.text() != text && !typing {
                        editor.set_text(cx, &text);
                        self.live_last_applied = text.clone();
                        self.live_last_good = text.clone();
                    }
                    self.live_key = key;
                    self.fn_external_seen = external;
                }
                let sw_ref = col.child(live_id!(big));
                let sw_opt = sw_ref.borrow_mut::<TweakMaterialSwatch>();
                if let Some(mut sw) = sw_opt {
                    sw.clip = self.swatch_clip;
                    if sw.applied_gen != self.rows_gen || sw.layer != layer || sw.mirror_uid != self.rows_uid {
                        let widget = cx.widget_tree().widget(WidgetUid(self.rows_uid));
                        if !widget.is_empty() {
                            sw.mirror_uid = widget.widget_uid().0;
                            sw.mirror_primary = self.materials.first().is_some_and(|m| *m == layer);
                            sw.mirror_layer = layer.clone();
                            sw.applied_gen = self.rows_gen;
                            sw.layer = layer;
                        }
                    }
                }
                // The animator's states, each a small posed well.
                self.refresh_state_swatches(cx, &col);
                // Under the wells: this layer's SHADER CONSTANTS, then its
                // INPUTS (uniforms/instances; annotated first), then source.
                {
                    let layer = self.vibe_layer.clone().unwrap_or_else(|| "draw_bg".to_string());
                    let mut ents = Vec::new();
                    let consts: Vec<usize> = self
                        .rows
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| r.const_ref.as_ref().is_some_and(|c| c.layer == layer))
                        .map(|(i, _)| i)
                        .collect();
                    if !consts.is_empty() {
                        ents.push(VisKind::TweakHeader(consts.len(), true));
                        for i in consts {
                            ents.push(VisKind::Tweakable(i));
                        }
                    }
                    let prefix = format!("{layer}.");
                    let mut inputs: Vec<usize> = self
                        .rows
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| {
                            r.const_ref.is_none()
                                && r.kind != RowKind::Info
                                && r.section != SectionKind::Cascade
                                && r.prop.starts_with(&prefix)
                        })
                        .map(|(i, _)| i)
                        .collect();
                    inputs.sort_by_key(|i| if self.row_docs.contains_key(&self.rows[*i].prop) { 0 } else { 1 });
                    if !inputs.is_empty() {
                        ents.push(VisKind::InputsHeader(inputs.len()));
                        for i in inputs {
                            ents.push(VisKind::Prop(i));
                        }
                    }
                    col.child(live_id!(shader_rows_wrap)).set_visible(cx, !ents.is_empty());
                    self.shader_entries = ents;
                }
            }
            // Tree tab data: refresh on generation change.
            if tab == PanelTab::Tree && self.tree_rows_gen != self.rows_gen.wrapping_add(1)
            {
                self.tree_rows = cx.widget_tree().flat_tree(cx);
                self.tree_rows_gen = self.rows_gen.wrapping_add(1);
                // Parent links from the depth-first order: the nearest
                // earlier row one level up.
                let mut stack: Vec<usize> = Vec::new();
                self.tree_parents = self
                    .tree_rows
                    .iter()
                    .enumerate()
                    .map(|(i, row)| {
                        while let Some(&top) = stack.last() {
                            if self.tree_rows[top].depth >= row.depth {
                                stack.pop();
                            } else {
                                break;
                            }
                        }
                        let parent = stack.last().copied();
                        stack.push(i);
                        parent
                    })
                    .collect();
                self.tree_children = vec![Vec::new(); self.tree_rows.len()];
                for (i, parent) in self.tree_parents.iter().enumerate() {
                    if let Some(parent) = parent {
                        self.tree_children[*parent].push(i);
                    }
                }
                self.tree_open_defaults_pending = true;
            }
        }

        self.composite_fields.clear();
        self.composite_align.clear();
        self.composite_clicks.clear();
        self.open_popup = None;
        session().lock().unwrap().popup = None;
        let entries_all = self.build_visible();
        let shader_entries = self.shader_entries.clone();
        let mut visible_rects: Vec<VisRow> = Vec::with_capacity(entries_all.len());

        let walk = Walk::abs_rect(Rect {
            pos: dvec2(band.pos.x + SPLITTER_WIDTH, band.pos.y),
            size: dvec2(band.size.x - SPLITTER_WIDTH, band.size.y),
        });
        self.tree_visible.clear();
        while let Some(step_widget) = sidebar.draw_walk(cx, scope, walk).step() {
            // The tree tab's list fills from the flattened hierarchy.
            if step_widget.widget_uid().0 == self.tree_list_uid {
                let sel_uid = self.rows_uid;
                // Filter: matching nodes plus their ancestor chain stay
                // visible; their folders force open while filtering.
                let filtering = !self.filter.is_empty();
                let keep: Option<Vec<bool>> = if filtering {
                    let mut keep = vec![false; self.tree_rows.len()];
                    for (i, row) in self.tree_rows.iter().enumerate() {
                        if row.name.to_lowercase().contains(&self.filter)
                            || row.ty.to_lowercase().contains(&self.filter)
                        {
                            let mut cursor = Some(i);
                            while let Some(index) = cursor {
                                if keep[index] {
                                    break;
                                }
                                keep[index] = true;
                                cursor = self.tree_parents.get(index).copied().flatten();
                            }
                        }
                    }
                    Some(keep)
                } else {
                    None
                };
                let Some(mut tree) = step_widget.borrow_mut::<FileTree>() else {
                    continue;
                };
                // First fill (or selection change): open the levels that
                // make the tree readable / reveal the selection.
                if self.tree_open_defaults_pending {
                    self.tree_open_defaults_pending = false;
                    for row in &self.tree_rows {
                        if row.has_children && row.depth < 4 {
                            tree.set_folder_is_open(cx, LiveId(row.uid), true, Animate::No);
                        }
                    }
                }
                if self.tree_scrolled_uid != sel_uid && sel_uid != 0 {
                    // The pin (by any route: body click, 3D pick, remote
                    // uid apply, undo) IS the tree selection.
                    tree.select_node(cx, LiveId(sel_uid));
                    if let Some(index) =
                        self.tree_rows.iter().position(|row| row.uid == sel_uid)
                    {
                        let mut cursor = self.tree_parents.get(index).copied().flatten();
                        while let Some(parent) = cursor {
                            tree.set_folder_is_open(
                                cx,
                                LiveId(self.tree_rows[parent].uid),
                                true,
                                Animate::No,
                            );
                            cursor = self.tree_parents.get(parent).copied().flatten();
                        }
                        // Scroll into view: the servo below measures and
                        // corrects on the next draws.
                        self.tree_scroll_tries = Some(8);
                    }
                    self.tree_scrolled_uid = sel_uid;
                }
                if self.tree_scroll_tries.is_some() && sel_uid != 0 {
                    tree.begin_reveal(LiveId(sel_uid));
                }
                if filtering {
                    if let Some(keep) = &keep {
                        for (i, row) in self.tree_rows.iter().enumerate() {
                            if keep[i] && row.has_children {
                                tree.set_folder_is_open(
                                    cx,
                                    LiveId(row.uid),
                                    true,
                                    Animate::No,
                                );
                            }
                        }
                    }
                }
                // Recursive emission over the flattened rows.
                fn emit(
                    tree: &mut FileTree,
                    cx: &mut Cx2d,
                    rows: &[crate::widget_tree::FlatTreeRow],
                    children: &[Vec<usize>],
                    keep: Option<&Vec<bool>>,
                    index: usize,
                ) {
                    if let Some(keep) = keep {
                        if !keep[index] {
                            return;
                        }
                    }
                    let row = &rows[index];
                    let label = format!("{} \u{00b7} {}", row.name, row.ty);
                    if row.has_children {
                        if tree.begin_folder(cx, LiveId(row.uid), &label).is_ok() {
                            for &child in &children[index] {
                                emit(tree, cx, rows, children, keep, child);
                            }
                            tree.end_folder();
                        }
                    } else {
                        tree.file(cx, LiveId(row.uid), &label);
                    }
                }
                for index in 0..self.tree_rows.len() {
                    if self.tree_parents[index].is_none() {
                        emit(
                            &mut tree,
                            cx,
                            &self.tree_rows,
                            &self.tree_children,
                            keep.as_ref(),
                            index,
                        );
                    }
                }
                if let Some(tries) = self.tree_scroll_tries {
                    let vp = self.props_viewport.unwrap_or(band);
                    match tree.take_reveal_y() {
                        Some(y) if tries > 0 => {
                            let top = vp.pos.y + 8.0;
                            let bottom = vp.pos.y + vp.size.y - 40.0;
                            if y < top || y > bottom {
                                tree.scroll_by(cx, y - (vp.pos.y + vp.size.y * 0.33));
                                tree.redraw(cx);
                                self.tree_scroll_tries = Some(tries - 1);
                            } else {
                                self.tree_scroll_tries = None;
                            }
                        }
                        _ => {
                            self.tree_scroll_tries = None;
                        }
                    }
                }
                continue;
            }
            let in_shader_tab = step_widget.widget_uid().0 == self.shader_list_uid;
            let list_viewport = self.props_viewport;
            let Some(mut list) = step_widget.borrow_mut::<PortalList>() else {
                continue;
            };
            let entries = if in_shader_tab { &shader_entries } else { &entries_all };
            list.set_item_range(cx, 0, entries.len());
            // A row can draw twice (TWEAKABLES + its home section); every
            // draw registers its fields, so the sets start empty per frame.
            for row in &mut self.rows {
                row.comp_uids.clear();
                row.mode_uids.clear();
                row.alt_uids.clear();
            }
            while let Some(entry_id) = list.next_visible_item(cx) {
                if entry_id >= entries.len() {
                    continue;
                }
                let entry = entries[entry_id];
                let template = match entry {
                    VisKind::Section(..) => live_id!(SectionRow),
                    VisKind::More(..) => live_id!(MoreRow),
                    VisKind::Material(_) => live_id!(MaterialRow),
                    VisKind::Size => live_id!(SizeRow),
                    VisKind::BoxInset(_) => live_id!(BoxRow),
                    VisKind::FlowSpacing => live_id!(FlowRow),
                    VisKind::AlignGrid => live_id!(AlignRow),
                    VisKind::TweakHeader(..) | VisKind::InputsHeader(_) => live_id!(SectionRow),
                    VisKind::CascadeLevel(_) => live_id!(CascadeRow),
                    VisKind::Prop(index) | VisKind::Tweakable(index) => match self.rows[index].struct_kind {
                        StructKind::Vec2 | StructKind::Vec3 | StructKind::Vec4 => live_id!(VecRow),
                        StructKind::Inset => live_id!(InsetRow),
                        StructKind::Metrics => live_id!(MetricsRow),
                        StructKind::SizeField => live_id!(SizeFieldRow),
                        StructKind::NoEditor => live_id!(NoEditorRow),
                        StructKind::None => match self.rows[index].kind {
                            RowKind::Num => live_id!(NumRow),
                            RowKind::Bool => live_id!(BoolRow),
                            RowKind::Color => live_id!(ColorRow),
                            RowKind::Text => live_id!(TextRow),
                            RowKind::Info => live_id!(InfoRow),
                        },
                    },
                };
                let (item, existed) = list.item_with_existed(cx, entry_id, template);
                if item.is_empty() {
                    continue;
                }
                match entry {
                    VisKind::Section(section, count, open) => {
                        let title = if self.panel_tab == PanelTab::Theme {
                            match section {
                                SectionKind::Style => "Colours",
                                SectionKind::Layout => "Spacing & sizes",
                                SectionKind::Text => "Font sizes",
                                _ => section.title(),
                            }
                        } else {
                            section.title()
                        };
                        item.child(live_id!(title)).set_text(cx, title);
                        item.child(live_id!(count))
                            .set_text(cx, &format!("{count}{}", if open { "" } else { "  +" }));
                    }
                    VisKind::More(_, count) => {
                        item.child(live_id!(title)).set_text(cx, &format!("+ show all ({count})"));
                        item.child(live_id!(count)).set_text(cx, "");
                    }
                    VisKind::TweakHeader(count, open) => {
                        item.child(live_id!(title)).set_text(cx, "Shader constants");
                        item.child(live_id!(count))
                            .set_text(cx, &format!("{count}{}", if open { "" } else { "  +" }));
                    }
                    VisKind::InputsHeader(count) => {
                        item.child(live_id!(title)).set_text(cx, "Inputs");
                        item.child(live_id!(count)).set_text(cx, &format!("{count}"));
                    }
                    VisKind::CascadeLevel(level) => {
                        if let Some(lvl) = self.cascade.get(level).cloned() {
                            let kind = cascade_icon_kind(&lvl.file);
                            let head = item.child(live_id!(head));
                            for (k, id) in [live_id!(ic_app), live_id!(ic_lib), live_id!(ic_theme), live_id!(ic_native)].into_iter().enumerate() {
                                head.child(id).set_visible(cx, k == kind);
                            }
                            let mut chip = head.child(live_id!(chip));
                            chip.child(live_id!(lbl)).set_text(cx, &format!("L{level}"));
                            let color = Self::level_color(level);
                            script_apply_eval!(cx, chip, { draw_bg +: { color: #(color) } });
                            head.child(live_id!(loc)).set_text(cx, if lvl.file.is_empty() { "native" } else { &lvl.loc });
                            let own: Vec<&str> = lvl.sets.iter().map(|(n, _)| n.as_str()).collect();
                            let over: Vec<&str> = lvl.sets.iter().filter(|(_, o)| *o).map(|(n, _)| n.as_str()).collect();
                            item.child(live_id!(sets_wrap)).set_visible(cx, !own.is_empty());
                            item.child(live_id!(overridden_wrap)).set_visible(cx, !over.is_empty());
                            item.child(live_id!(sets_wrap)).child(live_id!(sets)).set_text(cx, &if own.is_empty() { String::new() } else { format!("sets {}", own.join(" \u{00b7} ")) });
                            item.child(live_id!(overridden_wrap)).child(live_id!(overridden)).set_text(cx, &if over.is_empty() {
                                String::new()
                            } else {
                                // the closer level that wins each one
                                let mut by: Vec<String> = Vec::new();
                                for name in &over {
                                    let at = self.origin_levels.get(*name).map(|l| format!("L{l}")).unwrap_or_default();
                                    by.push(if at.is_empty() { name.to_string() } else { format!("{name} \u{2192} {at}") });
                                }
                                format!("overridden {}", by.join(" \u{00b7} "))
                            });
                        }
                    }
                    VisKind::Material(mi) => {
                        let layer = self.materials.get(mi).cloned().unwrap_or_default();
                        item.child(live_id!(name)).set_text(cx, &layer);
                        let sw_ref = item.child(live_id!(swatch_bg)).child(live_id!(swatch));
                        let sw_opt = sw_ref.borrow_mut::<TweakMaterialSwatch>();
                        if let Some(mut sw) = sw_opt {
                            sw.clip = list_viewport;
                            if sw.applied_gen != self.rows_gen || sw.layer != layer || sw.mirror_uid != self.rows_uid {
                                let widget =
                                    cx.widget_tree().widget(WidgetUid(self.rows_uid));
                                if !widget.is_empty() {
                                    sw.mirror_uid = widget.widget_uid().0;
                                    sw.mirror_primary = self.materials.first().is_some_and(|m| *m == layer);
                                    sw.mirror_layer = layer.clone();
                                    sw.applied_gen = self.rows_gen;
                                    sw.layer = layer;
                                }
                            }
                        }
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
                                btn.set_text(cx, labels[i]);
                                set_button_fill(cx, btn.clone(), i == active);
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
                    VisKind::Prop(index) | VisKind::Tweakable(index) => {
                        let as_tweakable = matches!(entry, VisKind::Tweakable(_));
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
                            } else if self.row_docs.contains_key(&self.rows[index].prop) {
                                // annotated: the author meant this one to be tweaked
                                vec4(0.86, 0.80, 0.58, 1.0)
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
                        let sk = self.rows[index].struct_kind;
                        if sk != StructKind::None {
                            match sk {
                                StructKind::Vec2 | StructKind::Vec3 | StructKind::Vec4 => {
                                    let n = match sk {
                                        StructKind::Vec2 => 2,
                                        StructKind::Vec3 => 3,
                                        _ => 4,
                                    };
                                    for (c, cid) in [live_id!(vx), live_id!(vy), live_id!(vz), live_id!(vw)]
                                        .into_iter()
                                        .enumerate()
                                    {
                                        let f = item.child(cid);
                                        if c == 2 {
                                            item.child(live_id!(vz_wrap)).set_visible(cx, c < n);
                                        } else if c == 3 {
                                            item.child(live_id!(vw_wrap)).set_visible(cx, c < n);
                                        }
                                        if c < n {
                                            self.rows[index].comp_uids.push(f.widget_uid().0);
                                            let v = self.rows[index].comp_vals.get(c).copied().unwrap_or(0.0);
                                            let input_opt = f.borrow_mut::<FabValueInput>();
                                            if let Some(mut input) = input_opt {
                                                input.set_value(cx, v);
                                            }
                                        }
                                    }
                                }
                                StructKind::Inset => {
                                    for (c, cid) in [live_id!(il), live_id!(it), live_id!(ir), live_id!(ib)]
                                        .into_iter()
                                        .enumerate()
                                    {
                                        let f = item.child(cid);
                                        self.rows[index].comp_uids.push(f.widget_uid().0);
                                        let v = self.rows[index].comp_vals.get(c).copied().unwrap_or(0.0);
                                        let input_opt = f.borrow_mut::<FabValueInput>();
                                        if let Some(mut input) = input_opt {
                                            input.set_value(cx, v);
                                        }
                                    }
                                }
                                StructKind::Metrics => {
                                    for (c, cid) in [live_id!(m0), live_id!(m1), live_id!(m2)]
                                        .into_iter()
                                        .enumerate()
                                    {
                                        let f = item.child(cid);
                                        self.rows[index].comp_uids.push(f.widget_uid().0);
                                        let v = self.rows[index].comp_vals.get(c).copied().unwrap_or(0.0);
                                        let input_opt = f.borrow_mut::<FabValueInput>();
                                        if let Some(mut input) = input_opt {
                                            input.set_value(cx, v);
                                        }
                                    }
                                }
                                StructKind::SizeField => {
                                    self.rows[index]
                                        .mode_uids
                                        .push(item.child(live_id!(sf_fill)).widget_uid().0);
                                    self.rows[index]
                                        .mode_uids
                                        .push(item.child(live_id!(sf_fit)).widget_uid().0);
                                    let f = item.child(live_id!(sf_num));
                                    self.rows[index].comp_uids.push(f.widget_uid().0);
                                }
                                StructKind::NoEditor | StructKind::None => {}
                            }
                        } else {
                        let field = item.child(live_id!(value));
                        if as_tweakable {
                            self.rows[index].alt_uids.push((field.widget_uid().0, false));
                        } else {
                            self.rows[index].field_uid = field.widget_uid().0;
                        }
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
                                if as_tweakable {
                                    self.rows[index].alt_uids.push((swatch.widget_uid().0, true));
                                } else {
                                    self.rows[index].swatch_uid = swatch.widget_uid().0;
                                }
                                let rgba = parse_hex(&self.rows[index].value);
                                // Theme mapping: a value that IS a theme
                                // colour names it, and one click makes the
                                // property say `theme.color_x` in splash.
                                // (a theme row IS its colour: no chip there)
                                let matched = if self.panel_tab == PanelTab::Theme {
                                    None
                                } else {
                                    rgba.and_then(|(c, _)| {
                                        let packed = packed_of(c);
                                        self.theme_colors.iter().find(|(_, tc, _)| *tc == packed).map(|(n, _, _)| n.clone())
                                    })
                                };
                                let wrap = item.child(live_id!(tname_wrap));
                                wrap.set_visible(cx, matched.is_some());
                                if let Some(name) = &matched {
                                    let btn = wrap.child(live_id!(tname));
                                    btn.set_text(cx, &format!("\u{2248} theme.{name}"));
                                    self.rows[index].theme_uid = btn.widget_uid().0;
                                }
                                self.rows[index].theme_match = matched;
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
                                                session().lock().unwrap().popup = Some(rect);
                                            }
                                        }
                                    }
                                }
                                drop(swatch);
                            }
                        }
                        }
                    }
                }
                item.draw_all(cx, &mut Scope::empty());
                // The colour popover's rect is known once it has drawn:
                // input priority for the popup, and /tweak/state's `popup`.
                if let Some(pick) = item.child(live_id!(swatch)).borrow::<FabColorPick>() {
                    let rect = pick.popover_rect();
                    if rect.size.x > 0.0 {
                        self.open_popup = Some(rect);
                        session().lock().unwrap().popup = Some(rect);
                    }
                }
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
    /// One small well per animator track of the pinned widget, under the
    /// main well: the same byte-copied draw call, posed by that track's
    /// off/on apply values and cycled on the frame clock. Every scrub or
    /// fn edit shows in all of them at once — they mirror the live call.
    fn refresh_state_swatches(&mut self, cx: &mut Cx, col: &WidgetRef) {
        let layer = self.vibe_layer.clone().unwrap_or_else(|| "draw_bg".to_string());
        let row = col.child(live_id!(states_row));
        let widget = cx.widget_tree().widget(WidgetUid(self.rows_uid));
        if widget.is_empty() {
            row.set_visible(cx, false);
            self.state_tracks.clear();
            return;
        }
        if self.states_gen != self.rows_gen || self.states_uid != self.rows_uid || self.states_layer != layer {
            self.state_tracks = animator_tracks(cx, &widget, &layer);
            self.states_gen = self.rows_gen;
            if self.states_uid != self.rows_uid {
                // A new selection starts its cycle at the off pose.
                self.states_t0 = cx.seconds_since_app_start();
            }
            self.states_uid = self.rows_uid;
            self.states_layer = layer.clone();
            session().lock().unwrap().state_names = self.state_tracks.iter().map(|t| t.group.clone()).collect();
        }
        row.set_visible(cx, !self.state_tracks.is_empty());
        let primary = self.materials.first().is_some_and(|m| *m == layer);
        let slots = [live_id!(st0), live_id!(st1), live_id!(st2), live_id!(st3), live_id!(st4), live_id!(st5)];
        for (i, slot) in slots.into_iter().enumerate() {
            let item = row.child(slot);
            match self.state_tracks.get(i) {
                Some(track) => {
                    item.set_visible(cx, true);
                    let mut label = format!(
                        "{} \u{00b7} in {}s, out {}s",
                        track.group,
                        fmt_f64(play_duration(track.on_play)),
                        fmt_f64(play_duration(track.off_play))
                    );
                    if let Some(doc) = self.row_docs.get(&format!("animator.{}", track.group)) {
                        let first = doc.lines().next().unwrap_or("").trim();
                        if !first.is_empty() {
                            label.push_str(" \u{00b7} ");
                            label.push_str(first);
                        }
                    }
                    item.child(live_id!(lbl)).set_text(cx, &label);
                    if let Some(mut sw) = item.child(live_id!(sw)).borrow_mut::<TweakMaterialSwatch>() {
                        sw.clip = self.swatch_clip;
                        sw.mirror_uid = self.rows_uid;
                        sw.mirror_primary = primary;
                        sw.mirror_layer = layer.clone();
                        sw.layer = layer.clone();
                        sw.applied_gen = self.rows_gen;
                        sw.state = Some(track.clone());
                    }
                }
                None => item.set_visible(cx, false),
            }
        }
        let pause = row.child(live_id!(states_pause));
        self.states_pause_uid = pause.widget_uid().0;
        pause.set_text(cx, if self.states_paused { "play" } else { "pause" });
        if !self.state_tracks.is_empty() {
            self.states_frame = cx.new_next_frame();
        }
    }

    /// One frame of the state swatches' cycle.
    fn states_tick(&mut self, cx: &mut Cx) {
        if self.state_tracks.is_empty() || !tweak_is_on() {
            return;
        }
        let Some(sidebar) = self.sidebar.clone() else { return };
        let col = sidebar.child(live_id!(shader_col));
        if !col.visible() {
            return;
        }
        let lock = session().lock().unwrap().states_lock;
        let paused = self.states_paused || self.states_hover;
        let t = cx.seconds_since_app_start() - self.states_t0;
        let row = col.child(live_id!(states_row));
        let slots = [live_id!(st0), live_id!(st1), live_id!(st2), live_id!(st3), live_id!(st4), live_id!(st5)];
        let mut moved = false;
        for (i, slot) in slots.into_iter().enumerate() {
            let Some(track) = self.state_tracks.get(i) else { break };
            let mix = match lock {
                Some(v) => v as f32,
                None if paused => continue,
                None => track_mix(track, t),
            };
            if let Some(mut sw) = row.child(slot).child(live_id!(sw)).borrow_mut::<TweakMaterialSwatch>() {
                if (sw.mix - mix).abs() > 1.0e-4 {
                    sw.mix = mix;
                    moved = true;
                }
            }
        }
        if moved {
            self.redraw_sidebar(cx);
        }
        if lock.is_none() && !paused {
            self.states_frame = cx.new_next_frame();
        }
    }

    /// Hovering an "≈ theme.color_x" chip pulses that colour everywhere it
    /// is drawn, live — leave restores by redrawing all (the ordinary draw
    /// path rebuilds every buffer; the pulse never touches the ledger).
    fn pulse_hover(&mut self, cx: &mut Cx, abs: Vec2d) {
        let mut over: Option<u32> = None;
        if tweak_is_on() {
            for row in &self.rows {
                if row.theme_uid == 0 {
                    continue;
                }
                let Some(name) = &row.theme_match else { continue };
                let rect = cx
                    .widget_tree()
                    .widget(WidgetUid(row.theme_uid))
                    .area()
                    .clipped_rect(cx);
                if rect.size.x > 0.0 && rect.contains(abs) {
                    over = self.theme_colors.iter().find(|(n, _, _)| n == name).map(|(_, c, _)| *c);
                    break;
                }
            }
        }
        // A remotely pinned pulse ignores the pointer until it is cleared.
        if self.pulse_pinned && over.is_none() {
            return;
        }
        self.set_pulse(cx, over);
    }

    /// Start pulsing `over` app-wide, hop to it from another colour, or
    /// (None) restore the true colour everywhere. Never touches the ledger.
    fn set_pulse(&mut self, cx: &mut Cx, over: Option<u32>) {
        match (over, self.pulse) {
            (Some(c), Some((p, _))) if c == p => {}
            (Some(c), _) => {
                // Hopping between chips restores the old colour first.
                self.pulse_end(cx);
                self.pulse = Some((c, cx.seconds_since_app_start()));
                self.pulse_ticks = 0;
                session().lock().unwrap().pulse = Some(PulseState::new(c));
                hook_sync(cx);
                self.pulse_frame = cx.new_next_frame();
                log!("TWEAK pulse on #{c:08x}");
            }
            (None, Some(_)) => {
                self.pulse_end(cx);
                log!("TWEAK pulse off — restore");
            }
            (None, None) => {}
        }
    }

    /// The theme palette for a colour popover's strip: greys first (by
    /// lightness), then by hue and lightness, so related colours sit
    /// together and the name under the strip says which one is which.
    fn palette_entries(&self) -> Vec<(String, [f32; 4])> {
        let mut entries: Vec<(String, [f32; 4], (u8, u16, u16))> = self
            .theme_colors
            .iter()
            .map(|(name, c, _)| {
                let rgba = [
                    ((c >> 24) & 0xff) as f32 / 255.0,
                    ((c >> 16) & 0xff) as f32 / 255.0,
                    ((c >> 8) & 0xff) as f32 / 255.0,
                    (c & 0xff) as f32 / 255.0,
                ];
                let [h, s, v] = rgb_to_hsv(rgba[0], rgba[1], rgba[2]);
                let key = if s < 0.1 {
                    (0u8, (v * 1000.0) as u16, (rgba[3] * 1000.0) as u16)
                } else {
                    (1u8, (h * 12.0) as u16, (v * 1000.0) as u16)
                };
                (name.clone(), rgba, key)
            })
            .collect();
        entries.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)));
        entries.into_iter().map(|(n, c, _)| (n, c)).collect()
    }

    /// Stop the pulse: true colours back into every slot it wrote, the
    /// hook down, and a redraw of everything for good measure.
    fn pulse_end(&mut self, cx: &mut Cx) {
        self.pulse = None;
        let st = session().lock().unwrap().pulse.take();
        hook_sync(cx);
        if let Some(st) = st {
            pulse_restore(cx, &st);
            cx.redraw_all();
        }
    }

    /// Resting the pointer on the state swatches pauses them.
    fn states_hover(&mut self, cx: &mut Cx, abs: Vec2d) {
        if self.state_tracks.is_empty() {
            return;
        }
        let Some(sidebar) = self.sidebar.as_ref() else { return };
        let row = sidebar.child(live_id!(shader_col)).child(live_id!(states_row));
        let rect = row.area().clipped_rect(cx);
        let over = rect.size.x > 0.0 && rect.contains(abs) && tweak_is_on();
        if over != self.states_hover {
            self.states_hover = over;
            if !over {
                self.states_frame = cx.new_next_frame();
            }
        }
    }

    /// The terse doc line under the layer name shows the whole doc while
    /// the pointer rests on it.
    fn doc_tip_hover(&mut self, cx: &mut Cx, abs: Vec2d) {
        let Some(sidebar) = self.sidebar.as_ref() else { return };
        let col = sidebar.child(live_id!(shader_col));
        let over = if self.shader_doc_full.is_empty() || !tweak_is_on() {
            false
        } else {
            // A label's area is its first glyph: the union is the line.
            let rect = col.child(live_id!(shader_doc)).area().clipped_rect_union(cx);
            rect.size.x > 0.0 && rect.contains(abs)
        };
        if over == self.doc_tip_shown {
            return;
        }
        self.doc_tip_shown = over;
        let tip = col.child(live_id!(doc_tip));
        let Some(mut tip) = tip.borrow_mut::<Tooltip>() else { return };
        if over {
            let rect = col.child(live_id!(shader_doc)).area().clipped_rect_union(cx);
            let pos = dvec2(rect.pos.x, rect.pos.y + rect.size.y + 2.0);
            tip.show_with_options(cx, pos, &self.shader_doc_full);
        } else {
            tip.hide(cx);
        }
    }

    /// Live code: apply the editor's text as it stands; a compile error
    /// puts the last good text back (the app never shows a blank widget)
    /// and says why under the editor.
    fn live_apply(&mut self, cx: &mut Cx) {
        let Some(sidebar) = self.sidebar.as_ref() else { return };
        let editor = sidebar
            .child(live_id!(shader_col))
            .child(live_id!(src_scroll))
            .child(live_id!(shader_src));
        let text = editor.text();
        if text == self.live_last_applied || !text.contains("fn") {
            return;
        }
        let _ = makepad_platform::shader_error::take();
        self.live_last_applied = text.clone();
        if let Err(error) = apply_fn_edit(cx, self, &text) {
            self.live_revert(cx, &error);
            return;
        }
        match makepad_platform::shader_error::take() {
            Some(err) => self.live_revert(cx, &err),
            None => {
                self.live_last_good = text;
                session().lock().unwrap().vibe_status = "live \u{2713}".to_string();
                self.live_error_pending = true;
                self.next_frame = cx.new_next_frame();
                self.redraw_sidebar(cx);
            }
        }
    }

    fn live_revert(&mut self, cx: &mut Cx, err: &str) {
        let first = err.trim().trim_start_matches("splash error:").trim();
        let first = first.split("; ").next().unwrap_or(first);
        let first = match first.find(" (from:") {
            Some(at) => &first[..at],
            None => first,
        };
        let first: String = first.chars().take(140).collect();
        session().lock().unwrap().vibe_status = format!("shader error: {first}");
        log!("TWEAK live shader error: {err}");
        if !self.live_last_good.is_empty() && self.live_last_good != self.live_last_applied {
            let good = self.live_last_good.clone();
            let _ = apply_fn_edit(cx, self, &good);
            let _ = makepad_platform::shader_error::take();
        }
        self.redraw_sidebar(cx);
    }

    /// The apply chunk for a change to one component of a structured row:
    /// a vec re-emits the whole vector (its components live together), an
    /// inset/metrics writes just the touched dotted sub-key.
    fn struct_component_chunk(&mut self, index: usize, comp: usize, v: f64) -> Option<String> {
        let kind = self.rows.get(index)?.struct_kind;
        let prop = self.rows.get(index)?.prop.clone();
        match kind {
            StructKind::Vec2 | StructKind::Vec3 | StructKind::Vec4 => {
                if let Some(slot) = self.rows[index].comp_vals.get_mut(comp) {
                    *slot = v;
                }
                let (n, pfx) = match kind {
                    StructKind::Vec2 => (2, "vec2f"),
                    StructKind::Vec3 => (3, "vec3f"),
                    _ => (4, "vec4f"),
                };
                let vals: Vec<String> = self.rows[index]
                    .comp_vals
                    .iter()
                    .take(n)
                    .map(|x| fmt_f64(*x))
                    .collect();
                Some(format!("{prop}: {pfx}({})", vals.join(" ")))
            }
            StructKind::Inset => {
                let key = ["left", "top", "right", "bottom"].get(comp)?;
                Some(format!("{prop}.{key}: {}", fmt_f64(v)))
            }
            StructKind::Metrics => {
                let key = ["descender", "line_gap", "line_scale"].get(comp)?;
                Some(format!("{prop}.{key}: {}", fmt_f64(v)))
            }
            StructKind::SizeField => Some(format!("{prop}: {}", fmt_f64(v))),
            StructKind::NoEditor | StructKind::None => None,
        }
    }

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
        let prop = row.prop.clone();
        self.reset_prop(cx, sel, &prop);
    }

    /// Reset one prop to its session baseline (the FIRST diff entry's old
    /// value) and drop every ledger entry for it — a reset value is as if
    /// it was never touched: /tweak/state, /tweak/final and the changed
    /// indicators all forget it.
    fn reset_prop(&mut self, cx: &mut Cx, sel: &TweakPick, prop: &str) {
        let original = {
            let session = session().lock().unwrap();
            session
                .diff
                .iter()
                .find(|entry| entry.path == sel.path && entry.prop == prop)
                .map(|entry| entry.old.clone())
        };
        let Some(original) = original else {
            return; // untouched this session: nothing to reset
        };
        let prop = prop.to_string();
        let widget = cx.widget_tree().widget(WidgetUid(sel.uid));
        if widget.is_empty() {
            return;
        }
        let chunk = format!("{prop}: {original}");
        match eval_chunk(cx, &widget, &chunk) {
            Ok(()) => {
                let mut session = session().lock().unwrap();
                let removed: Vec<TweakDiffEntry> = session
                    .diff
                    .iter()
                    .filter(|entry| entry.path == sel.path && entry.prop == prop)
                    .cloned()
                    .collect();
                if !removed.is_empty() {
                    session.redo.clear();
                    session.undo.push(UndoStep::Reset {
                        path: sel.path.clone(),
                        prop: prop.clone(),
                        removed,
                    });
                    session.undo_open = false;
                }
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

    /// Cmd+Z: pop the top edit gesture — restore the pre-gesture value and
    /// remove the gesture's ledger entries, as if it never happened.
    fn undo(&mut self, cx: &mut Cx) {
        let Some(step) = session().lock().unwrap().undo.pop() else {
            return;
        };
        match &step {
            UndoStep::Value {
                path,
                prop,
                old,
                seq_start,
                ..
            } => {
                let undone = if path.as_str() == "theme" {
                    self.theme_set(cx, prop.as_str(), old.as_str(), "undo")
                } else {
                    let Ok(widget) = resolve_widget_for_history(cx, path) else {
                        session().lock().unwrap().undo.push(step);
                        return;
                    };
                    if let Some(name) = prop.strip_prefix("const:") {
                        const_set(cx, &widget, path, name, old.parse::<f64>().ok(), "undo").map(|_| ())
                    } else {
                        eval_chunk(cx, &widget, &format!("{prop}: {old}"))
                    }
                };
                if let Err(error) = undone {
                    log!("TWEAK undo failed: {error}");
                    return;
                }
                let mut s = session().lock().unwrap();
                s.diff.retain(|e| {
                    !(e.path == *path && e.prop == *prop && e.seq >= *seq_start)
                });
                s.apply_gen += 1;
                s.undo_open = false;
                drop(s);
                log!("TWEAK undo {} {} -> {}", path, prop, old);
            }
            UndoStep::Reset {
                path,
                prop,
                removed,
            } => {
                let Ok(widget) = resolve_widget_for_history(cx, path) else {
                    session().lock().unwrap().undo.push(step);
                    return;
                };
                let last = removed.last().map(|e| e.new.clone()).unwrap_or_default();
                let chunk = format!("{prop}: {last}");
                if let Err(error) = eval_chunk(cx, &widget, &chunk) {
                    log!("TWEAK undo(reset) failed: {error}");
                    return;
                }
                let mut s = session().lock().unwrap();
                for entry in removed {
                    s.diff.push(entry.clone());
                }
                s.apply_gen += 1;
                s.undo_open = false;
                drop(s);
                log!("TWEAK undo reset {} {} -> {}", path, prop, last);
            }
        }
        session().lock().unwrap().redo.push(step);
        self.rows_uid = 0;
        cx.redraw_all();
    }

    /// Cmd+Shift+Z: replay the most recently undone gesture.
    fn redo(&mut self, cx: &mut Cx) {
        let Some(step) = session().lock().unwrap().redo.pop() else {
            return;
        };
        match &step {
            UndoStep::Value {
                path, prop, new, ..
            } => {
                let redone = if path.as_str() == "theme" {
                    self.theme_set(cx, prop.as_str(), new.as_str(), "redo")
                } else {
                    let Ok(widget) = resolve_widget_for_history(cx, path) else {
                        session().lock().unwrap().redo.push(step);
                        return;
                    };
                    if let Some(name) = prop.strip_prefix("const:") {
                        const_set(cx, &widget, path, name, new.parse::<f64>().ok(), "redo").map(|_| ())
                    } else {
                        let chunk = format!("{prop}: {new}");
                        apply_splash_chunk(cx, &widget, path, &chunk, "redo").map(|_| ())
                    }
                };
                if let Err(error) = redone {
                    log!("TWEAK redo failed: {error}");
                    return;
                }
                let mut s = session().lock().unwrap();
                let seq_start = s
                    .diff
                    .last()
                    .map(|e| e.seq)
                    .unwrap_or(0);
                s.undo.push(UndoStep::Value {
                    path: path.clone(),
                    prop: prop.clone(),
                    old: match &step {
                        UndoStep::Value { old, .. } => old.clone(),
                        _ => unreachable!(),
                    },
                    new: new.clone(),
                    seq_start,
                });
                s.undo_open = false;
                drop(s);
                log!("TWEAK redo {} {} -> {}", path, prop, new);
            }
            UndoStep::Reset {
                path,
                prop,
                removed,
            } => {
                let Ok(widget) = resolve_widget_for_history(cx, path) else {
                    session().lock().unwrap().redo.push(step);
                    return;
                };
                let baseline = removed.first().map(|e| e.old.clone()).unwrap_or_default();
                let chunk = format!("{prop}: {baseline}");
                if let Err(error) = eval_chunk(cx, &widget, &chunk) {
                    log!("TWEAK redo(reset) failed: {error}");
                    return;
                }
                let mut s = session().lock().unwrap();
                let (path_c, prop_c) = (path.clone(), prop.clone());
                s.diff
                    .retain(|e| !(e.path == path_c && e.prop == prop_c));
                s.undo.push(step.clone());
                s.apply_gen += 1;
                s.undo_open = false;
                drop(s);
                log!("TWEAK redo reset {} {}", path_c, prop_c);
            }
        }
        self.rows_uid = 0;
        cx.redraw_all();
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
            if self.note_text_uid != 0 && widget_action.widget_uid.0 == self.note_text_uid {
                if let TextInputAction::Changed(text) = widget_action.cast::<TextInputAction>() {
                    let sel = session().lock().unwrap().pinned.clone();
                    if let Some(sel) = sel {
                        let mut s = session().lock().unwrap();
                        if let Some(note) = s.notes.iter_mut().find(|n| n.path == sel.path) {
                            note.text = text.clone();
                        }
                    }
                }
            }
            if self.vibe_prompt_uid != 0
                && widget_action.widget_uid.0 == self.vibe_prompt_uid
            {
                if let TextInputAction::Returned(text, _) = widget_action.cast::<TextInputAction>()
                {
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        let (path, layer) = {
                            let s = session().lock().unwrap();
                            (
                                s.pinned
                                    .as_ref()
                                    .map(|p| p.path.clone())
                                    .unwrap_or_default(),
                                self.vibe_layer.clone().unwrap_or_default(),
                            )
                        };
                        // The execute bundle, on the AI's ear (the TWEAK
                        // log + /tweak/state carry it to the driving
                        // agent): scope = exactly this draw layer.
                        // Code only: the fn sources (with their file:line)
                        // ride along; colours/sizes are the Props rows'.
                        let fns = self
                            .vibe_fn_sources
                            .iter()
                            .map(|(name, loc, src)| format!("// {name} \u{2014} {loc}\n{src}"))
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        log!("TWEAK vibe sel={path} layer={layer} fns={} prompt={text}", self.vibe_fn_sources.iter().map(|(n, l, _)| format!("{n}@{l}")).collect::<Vec<_>>().join(","));
                        {
                            let mut s = session().lock().unwrap();
                            s.vibe_status = format!("sent to the AI \u{00b7} waiting\u{2026} \u{2014} {text}");
                            s.vibe_pending = Some((path.clone(), layer.clone()));
                            s.vibes.push((path, layer, text, fns));
                        }
                        if let Some(sidebar) = self.sidebar.as_ref() {
                            let col = sidebar.child(live_id!(shader_col));
                            col.child(live_id!(prompt)).set_text(cx, "");
                            let status = session().lock().unwrap().vibe_status.clone();
                            col.child(live_id!(vibe_status)).set_text(cx, &status);
                        }
                        cx.redraw_all();
                    }
                }
            }
            if self.tab_uids.contains(&widget_action.widget_uid.0)
                && widget_action.widget_uid.0 != 0
            {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    let index = self
                        .tab_uids
                        .iter()
                        .position(|uid| *uid == widget_action.widget_uid.0)
                        .unwrap_or(0);
                    self.panel_tab = match index {
                        1 => PanelTab::Shader,
                        2 => PanelTab::Tree,
                        3 => PanelTab::Theme,
                        _ => PanelTab::Props,
                    };
                    self.redraw_sidebar(cx);
                }
            }
            if self.tree_list_uid != 0 && widget_action.widget_uid.0 == self.tree_list_uid {
                match widget_action.cast::<FileTreeAction>() {
                    FileTreeAction::FileClicked(id) | FileTreeAction::FolderClicked(id) => {
                        // Tree node click: pin that widget, exactly like a
                        // body pick (drives 2D outline AND the 3D view).
                        let target = id.0;
                        let widget = cx.widget_tree().widget(WidgetUid(target));
                        if !widget.is_empty() {
                            let rect = widget.area().clipped_rect_union(cx);
                            let ids = cx.widget_tree().path_to(WidgetUid(target));
                            let path = ids
                                .iter()
                                .map(|id| live_id_token(*id))
                                .collect::<Vec<_>>()
                                .join(".");
                            let ty = widget
                                .widget_type_id()
                                .and_then(|type_id| {
                                    widget_type_names(cx).get(&type_id).copied()
                                })
                                .map(live_id_token)
                                .unwrap_or_else(|| "-".to_string());
                            session().lock().unwrap().pinned = Some(TweakPick {
                                uid: target,
                                path,
                                ty,
                                rect,
                                window_id: self.my_window.unwrap_or(0),
                                band: None,
                                level: 0,
                            });
                            self.rows_uid = 0;
                            self.redraw_overlay(cx);
                            self.redraw_sidebar(cx);
                        }
                    }
                    FileTreeAction::NodeHovered(id) => {
                        // Tree hover: outline that widget in the body/3D.
                        let widget = cx.widget_tree().widget(WidgetUid(id.0));
                        if !widget.is_empty() {
                            let rect = widget.area().clipped_rect_union(cx);
                            if rect.size.x > 0.0 {
                                session().lock().unwrap().hover = Some(TweakPick {
                                    uid: id.0,
                                    path: String::new(),
                                    ty: String::new(),
                                    rect,
                                    window_id: self.my_window.unwrap_or(0),
                                    band: None,
                                    level: 0,
                                });
                                self.tree_hover_active = true;
                                self.redraw_overlay(cx);
                            }
                        }
                    }
                    FileTreeAction::NodeHoverEnded(_) => {
                        if self.tree_hover_active {
                            self.tree_hover_active = false;
                            session().lock().unwrap().hover = None;
                            self.redraw_overlay(cx);
                        }
                    }
                    _ => {}
                }
            }
            if self.sploded_uid != 0 && widget_action.widget_uid.0 == self.sploded_uid {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    // The 2.5D exploded z-layer view. Inspection-only while
                    // up (input belongs to the mode): exit with Esc or F10.
                    cx.sploded_toggle();
                    // The toggle is deferred to the next event; read the
                    // state it WILL have, not the one it still has.
                    self.sploded_armed = cx.sploded_will_be_active();
                    if let Some(sidebar) = self.sidebar.as_ref() {
                        let spread_wrap = sidebar.child(live_id!(filter_row)).child(live_id!(spread_wrap));
                        let spread = spread_wrap.child(live_id!(spread));
                        let spread_now = cx.sploded_spread() as f64;
                        if let Some(mut field) = spread.borrow_mut::<FabValueInput>() {
                            field.set_value(cx, spread_now);
                        };
                        spread_wrap.set_visible(cx, self.sploded_armed);
                    }
                    log!("TWEAK sploded view {}", if self.sploded_armed { "ON" } else { "off" });
                }
            }
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
                                .child(live_id!(filter_row))
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
        // The Theme tab edits the theme itself: no widget is selected.
        let theme = self.panel_tab == PanelTab::Theme;
        let Some(sel) = sel.or_else(|| theme.then(TweakPick::default)) else {
            return;
        };
        #[derive(Clone)]
        enum Edit {
            Apply(String),
            Revert(usize, String),
            HoldOn(usize),
            HoldOff,
            Eyedrop(String),
            /// Fill a just-opened colour popover's palette strip.
            Palette(u64),
            /// Pulse a theme colour by name (None: stop).
            PulseName(Option<String>),
        }
        let mut edits: Vec<Edit> = Vec::new();
        let mut resets: Vec<String> = Vec::new();
        let mut const_edits: Vec<(usize, Option<f64>)> = Vec::new();
        for action in actions {
            let Some(widget_action) = action.as_widget_action() else {
                continue;
            };
            let action_uid = widget_action.widget_uid.0;
            // The exploded view's spread knob: level separation, live.
            if self.shader_src_uid != 0 && action_uid == self.shader_src_uid {
                // The editor only reports document changes: live code
                // settles 200ms after the last keystroke.
                self.live_timer = cx.start_timeout(0.2);
                continue;
            }
            if action_uid != 0 && (action_uid == self.scope_this_uid || action_uid == self.scope_all_uid) {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    let all = action_uid == self.scope_all_uid;
                    session().lock().unwrap().scope_all = all;
                    log!("TWEAK scope {}", if all { "all — every widget of the type" } else { "this — specialise this instance" });
                    self.rows_uid = 0;
                    self.redraw_sidebar(cx);
                }
                continue;
            }
            if let Some((index, name)) = self.rows.iter().enumerate().find_map(|(i, b)| {
                if b.theme_uid != 0 && b.theme_uid == action_uid {
                    b.theme_match.clone().map(|n| (i, n))
                } else {
                    None
                }
            }) {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    edits.push(Edit::Apply(format!("{}: theme.{}", self.rows[index].prop, name)));
                }
                continue;
            }
            if self.states_pause_uid != 0 && action_uid == self.states_pause_uid {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    self.states_paused = !self.states_paused;
                    if !self.states_paused {
                        self.states_frame = cx.new_next_frame();
                    }
                    self.redraw_sidebar(cx);
                }
                continue;
            }
            if self.note_uid != 0 && action_uid == self.note_uid {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    self.note_request = true;
                    cx.redraw_all();
                }
                continue;
            }
            if self.shader_fold_uid != 0 && action_uid == self.shader_fold_uid {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    self.shader_src_open = !self.shader_src_open;
                    self.redraw_sidebar(cx);
                }
                continue;
            }
            if self.spread_uid != 0 && action_uid == self.spread_uid {
                match widget_action.cast::<FabValueInputAction>() {
                    FabValueInputAction::Changed(v) | FabValueInputAction::Ended(v) => {
                        cx.sploded_set_spread(v as f32);
                    }
                    FabValueInputAction::Reset => {
                        cx.sploded_set_spread(SPLODED_SPREAD_DEFAULT);
                        if let Some(sidebar) = self.sidebar.as_ref() {
                            let spread = sidebar
                                .child(live_id!(filter_row))
                                .child(live_id!(spread_wrap))
                                .child(live_id!(spread));
                            if let Some(mut field) = spread.borrow_mut::<FabValueInput>() {
                                field.set_value(cx, SPLODED_SPREAD_DEFAULT as f64);
                            };
                        }
                    }
                    _ => {}
                }
                continue;
            }
            // Composite-row fields (size pair, box legs, spacing, align
            // dots, link toggles) are not rows; resolve them first.
            if let Some((_, prop)) = self
                .composite_fields
                .iter()
                .find(|(uid, _)| *uid == action_uid)
                .cloned()
            {
                match widget_action.cast::<FabValueInputAction>() {
                    FabValueInputAction::Reset => {
                        resets.push(prop.clone());
                        continue;
                    }
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
            // A structured value's component field (vec x/y, inset leg,
            // metric): apply the whole value (vec) or the touched dotted
            // sub-key (inset/metrics), which the grammar accepts.
            if let Some((index, comp)) = self.rows.iter().enumerate().find_map(|(i, b)| {
                b.comp_uids
                    .iter()
                    .position(|u| *u == action_uid)
                    .map(|c| (i, c % comp_count(b.struct_kind)))
            }) {
                self.doc_row = Some(index);
                match widget_action.cast::<FabValueInputAction>() {
                    FabValueInputAction::Changed(v) => {
                        if let Some(chunk) = self.struct_component_chunk(index, comp, v) {
                            edits.push(Edit::Apply(chunk));
                        }
                    }
                    FabValueInputAction::Ended(_) => edits.push(Edit::HoldOff),
                    _ => {}
                }
                continue;
            }
            // A SizeField's Fill / Fit button.
            if let Some((index, is_fill)) = self.rows.iter().enumerate().find_map(|(i, b)| {
                b.mode_uids
                    .iter()
                    .position(|u| *u == action_uid)
                    .map(|p| (i, p % 2 == 0))
            }) {
                self.doc_row = Some(index);
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    let word = if is_fill { "Fill" } else { "Fit" };
                    edits.push(Edit::Apply(format!("{}: {}", self.rows[index].prop, word)));
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
                        || b.alt_uids.iter().any(|(u, _)| *u == action_uid)
                })
                .map(|(i, b)| (i, b.clone()))
            else {
                continue;
            };
            self.doc_row = Some(index);
            let is_swatch = (binding.swatch_uid != 0 && binding.swatch_uid == action_uid)
                || binding.alt_uids.iter().any(|(u, s)| *u == action_uid && *s);
            match binding.kind {
                // CASCADE rows are read-only labels; nothing to apply.
                RowKind::Info => {}
                RowKind::Num if binding.const_ref.is_some() => {
                    match widget_action.cast::<FabValueInputAction>() {
                        FabValueInputAction::Changed(v) => const_edits.push((index, Some(v))),
                        FabValueInputAction::Ended(v) => {
                            // A typed value arrives only as Ended.
                            if binding.value.parse::<f64>().ok() != Some(v) {
                                const_edits.push((index, Some(v)));
                            }
                            edits.push(Edit::HoldOff);
                        }
                        FabValueInputAction::Reset => const_edits.push((index, None)),
                        _ => {}
                    }
                }
                RowKind::Num => match widget_action.cast::<FabValueInputAction>() {
                    FabValueInputAction::Changed(v) => {
                        edits.push(Edit::Apply(format!("{}: {}", binding.prop, fmt_f64(v))));
                    }
                    FabValueInputAction::Ended(_) => {
                        edits.push(Edit::HoldOff);
                    }
                    FabValueInputAction::Reset => {
                        resets.push(binding.prop.clone());
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
                        edits.push(Edit::Palette(binding.swatch_uid));
                    }
                    FabColorPickAction::Closed => {
                        edits.push(Edit::HoldOff);
                    }
                    FabColorPickAction::Eyedropper => {
                        edits.push(Edit::HoldOff);
                        edits.push(Edit::Eyedrop(binding.prop.clone()));
                    }
                    FabColorPickAction::PaletteHover(name) => {
                        edits.push(Edit::PulseName(name));
                    }
                    FabColorPickAction::PalettePick(name) => {
                        // Bind by reference: the ledger says `theme.color_x`.
                        edits.push(Edit::HoldOff);
                        edits.push(Edit::PulseName(None));
                        edits.push(Edit::Apply(format!("{}: theme.{name}", binding.prop)));
                    }
                    _ => {}
                },
            }
        }
        for prop in resets {
            if theme {
                self.theme_reset(cx, &prop);
            } else {
                self.reset_prop(cx, &sel, &prop);
            }
        }
        for (index, value) in const_edits {
            let Some(cref) = self.rows.get(index).and_then(|r| r.const_ref.clone()) else { continue };
            let widget = cx.widget_tree().widget(WidgetUid(sel.uid));
            match const_set(cx, &widget, &sel.path, &cref.name, value, "sidebar") {
                Ok((_, new)) => {
                    if let Some(row) = self.rows.get_mut(index) {
                        row.value = fmt_f64(new as f64);
                        row.changed = new != cref.initial;
                    }
                    self.redraw_sidebar(cx);
                }
                Err(error) => log!("TWEAK shader constant apply failed: {error}"),
            }
        }
        for edit in edits {
            match edit {
                Edit::Apply(chunk) if theme => self.theme_apply_chunk(cx, &chunk),
                Edit::Apply(chunk) => self.sidebar_apply(cx, &sel, &chunk),
                Edit::Eyedrop(prop) => {
                    log!("TWEAK eyedropper armed for {prop} — click a pixel in the app");
                    session().lock().unwrap().eyedrop = Some(prop);
                    cx.set_cursor(MouseCursor::Crosshair);
                }
                Edit::Palette(uid) => {
                    let entries = self.palette_entries();
                    let swatch = cx.widget_tree().widget(WidgetUid(uid));
                    if let Some(mut pick) = swatch.borrow_mut::<FabColorPick>() {
                        pick.set_palette(cx, entries);
                    };
                }
                Edit::PulseName(name) => {
                    let color = name.and_then(|n| {
                        self.theme_colors.iter().find(|(k, _, _)| *k == n).map(|(_, c, _)| *c)
                    });
                    self.pulse_pinned = color.is_some();
                    self.set_pulse(cx, color);
                }
                Edit::Revert(index, value) => {
                    // Push the origin value back through the same path, then
                    // refresh the field itself.
                    if let Some(row) = self.rows.get(index) {
                        let text = if row.quoted {
                            format!("{value:?}")
                        } else {
                            value.clone()
                        };
                        let chunk = format!("{}: {}", row.prop, text);
                        if theme {
                            self.theme_apply_chunk(cx, &chunk);
                        } else {
                            self.sidebar_apply(cx, &sel, &chunk);
                        }
                    }
                    if !theme {
                        self.rows_uid = 0;
                    }
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
                    let mut s = session().lock().unwrap();
                    s.edit_hold = false;
                    // Gesture boundary: the next apply starts a NEW undo
                    // step (two scrubs on one prop never merge).
                    s.undo_open = false;
                    drop(s);
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
            let list_ref = sidebar.child(live_id!(props_wrap)).child(live_id!(props));
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
        if pick.rect.size.x <= 0.0 || pick.rect.size.y <= 0.0 {
            return;
        }
        let dpi = cx.current_dpi_factor().max(1.0) as f32;
        self.draw_outline.dpi = dpi;
        match style {
            PickStyle::Pinned => {
                // The SELECTION never wears a box: the whole point of
                // pinning a widget is seeing how it actually renders, and
                // an outline sits exactly on the edge pixels being judged.
                // Four viewfinder corners at a healthy distance mark the
                // selection and leave the widget — and the margin space
                // around it — untouched for direct manipulation.
                self.draw_corner_brackets(cx, pick.rect);
                return; // no outline, no fill, no label chip
            }
            PickStyle::Hover => {
                self.draw_outline.border_color = vec4(0.19, 0.78, 1.0, 1.0);
                self.draw_outline.fill_color = vec4(0.0, 0.0, 0.0, 0.0);
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

        // No pick banner: the panel footer carries the identity (the
        // full-path label used to stretch across the app).
    }

    /// The selection mark: four `⌐`-style corner brackets, outset from the
    /// pinned rect so the widget's own rendering (and the margin gutter the
    /// person is about to drag) stays completely visible.
    fn draw_corner_brackets(&mut self, cx: &mut Cx2d, rect: Rect) {
        const OUTSET: f64 = 5.0;
        const ARM: f64 = 9.0;
        const THICK: f64 = 2.0;
        let max_x = self.overlay_max_x(cx.current_pass_size());
        self.draw_outline.border_color = vec4(0.0, 0.0, 0.0, 0.0);
        self.draw_outline.border_size = 0.0;
        self.draw_outline.dash = 0.0;
        self.draw_outline.fill_color = vec4(0.19, 0.78, 1.0, 1.0);
        let left = rect.pos.x - OUTSET;
        let top = rect.pos.y - OUTSET;
        let right = rect.pos.x + rect.size.x + OUTSET;
        let bottom = rect.pos.y + rect.size.y + OUTSET;
        // (horizontal arm, vertical arm) per corner, arms pointing inward.
        let arms = [
            (dvec2(left, top), dvec2(ARM, THICK), dvec2(left, top), dvec2(THICK, ARM)),
            (
                dvec2(right - ARM, top),
                dvec2(ARM, THICK),
                dvec2(right - THICK, top),
                dvec2(THICK, ARM),
            ),
            (
                dvec2(left, bottom - THICK),
                dvec2(ARM, THICK),
                dvec2(left, bottom - ARM),
                dvec2(THICK, ARM),
            ),
            (
                dvec2(right - ARM, bottom - THICK),
                dvec2(ARM, THICK),
                dvec2(right - THICK, bottom - ARM),
                dvec2(THICK, ARM),
            ),
        ];
        for (h_pos, h_size, v_pos, v_size) in arms {
            for (pos, size) in [(h_pos, h_size), (v_pos, v_size)] {
                if pos.x + size.x > max_x {
                    continue; // never into the panel band
                }
                self.draw_outline.draw_abs(cx, Rect { pos, size });
            }
        }
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
        // An eyedropper sample in flight: apply it the moment the frame
        // has been read back, else look again next frame.
        let probe = session().lock().unwrap().eyedrop_probe.clone();
        if let Some((id, prop)) = probe {
            match makepad_platform::pixel_probe::take_pixel_probe(id) {
                Some(Some(rgba)) => {
                    session().lock().unwrap().eyedrop_probe = None;
                    let hex = format_hex(
                        [
                            rgba[0] as f32 / 255.0,
                            rgba[1] as f32 / 255.0,
                            rgba[2] as f32 / 255.0,
                            rgba[3] as f32 / 255.0,
                        ],
                        true,
                    );
                    log!("TWEAK eyedropper {prop} <- {hex}");
                    let sel = session().lock().unwrap().pinned.clone();
                    if let Some(sel) = sel {
                        self.sidebar_apply(cx, &sel, &format!("{prop}: {hex}"));
                        self.rows_uid = 0;
                    }
                    self.redraw_sidebar(cx);
                }
                Some(None) => {
                    self.next_frame = cx.new_next_frame();
                }
                None => {
                    session().lock().unwrap().eyedrop_probe = None;
                }
            }
        }
        if self.live_timer.is_event(event).is_some() {
            self.live_apply(cx);
        }
        // The guard must drop before undo/redo take the session lock again
        // (an `if let` scrutinee's temporary lives for the whole body).
        let pending_undo = session().lock().unwrap().undo_redo.take();
        if let Some(undo) = pending_undo {
            if undo {
                self.undo(cx);
            } else {
                self.redo(cx);
            }
        }
        let theme_req = session().lock().unwrap().theme_req.take();
        if let Some((name, value)) = theme_req {
            if let Err(error) = self.theme_set(cx, &name, &value, "remote") {
                log!("TWEAK theme set failed: {error}");
            }
        }
        let pulse_req = session().lock().unwrap().pulse_req.take();
        if let Some(req) = pulse_req {
            let color = if req.is_empty() {
                None
            } else if let Some(hex) = req.strip_prefix('#') {
                u32::from_str_radix(hex, 16).ok().map(|v| if hex.len() == 6 { (v << 8) | 0xff } else { v })
            } else {
                let found = self.theme_colors.iter().find(|(n, _, _)| *n == req).map(|(_, c, _)| *c);
                if found.is_none() {
                    log!("TWEAK pulse: unknown theme colour {req}");
                }
                found
            };
            self.pulse_pinned = color.is_some();
            self.set_pulse(cx, color);
        }
        if let Event::MouseMove(e) = event {
            self.doc_tip_hover(cx, e.abs);
            self.states_hover(cx, e.abs);
            self.pulse_hover(cx, e.abs);
        }
        if self.pulse_frame.is_event(event).is_some() {
            if let Some((_, t0)) = self.pulse {
                let now = cx.seconds_since_app_start();
                let t = now - t0;
                let lock = session().lock().unwrap().pulse_lock;
                let m = lock.unwrap_or_else(|| 0.3 + 0.25 * ((t * std::f64::consts::TAU * 2.5).sin() as f32));
                // Frames arrive far faster than the display on a hidden or
                // occluded window; pace the buffer work at ~90 Hz, and a
                // locked tone needs no work at all once it is on screen.
                let due = now - self.pulse_last_sync >= 1.0 / 90.0;
                let mut st = if due { session().lock().unwrap().pulse.take() } else { None };
                if st.as_ref().is_some_and(|s| lock.is_some() && s.m == m && self.pulse_ticks > 0) {
                    session().lock().unwrap().pulse = st.take();
                }
                if let Some(mut st) = st {
                    self.pulse_last_sync = now;
                    st.m = m;
                    let hits = pulse_sync(cx, &mut st);
                    pulse_repaint(cx, &st);
                    if self.pulse_ticks == 0 {
                        let n = |f: &dyn Fn(&PulseSlot) -> bool| st.ledger.iter().filter(|s| f(s)).count();
                        log!(
                            "TWEAK pulse hits {hits} (inst {} uni {} scope {} clear {})",
                            n(&|s| matches!(s, PulseSlot::Inst { .. })),
                            n(&|s| matches!(s, PulseSlot::Uni { .. })),
                            n(&|s| matches!(s, PulseSlot::Scope { .. })),
                            n(&|s| matches!(s, PulseSlot::Clear { .. }))
                        );
                    } else if self.pulse_ticks % 90 == 0 {
                        log!("TWEAK pulse tick {} m={m:.2} ledger {hits}", self.pulse_ticks);
                    }
                    session().lock().unwrap().pulse = Some(st);
                    self.pulse_ticks += 1;
                }
                self.pulse_frame = cx.new_next_frame();
            }
        }
        // The wells' scroll viewports, read between frames when the rect
        // slots hold the drawn values (mid-draw they answer zero).
        if tweak_is_on() {
            if let Some(sidebar) = self.sidebar.clone() {
                let col_rect = sidebar.child(live_id!(shader_col)).area().rect(cx);
                if col_rect.size.y > 0.0 {
                    self.swatch_clip = Some(col_rect);
                }
                let wrap = sidebar.child(live_id!(props_wrap));
                let wrap_rect = wrap.area().rect(cx);
                if wrap_rect.size.y > 0.0 {
                    let scope = wrap.child(live_id!(scope_row)).area().rect(cx);
                    let top = if scope.size.y > 0.0 { scope.pos.y + scope.size.y } else { wrap_rect.pos.y };
                    self.props_viewport = Some(Rect {
                        pos: dvec2(wrap_rect.pos.x, top),
                        size: dvec2(wrap_rect.size.x, (wrap_rect.pos.y + wrap_rect.size.y - top).max(0.0)),
                    });
                }
            }
        }
        if self.states_frame.is_event(event).is_some() {
            self.states_tick(cx);
        }
        if self.live_error_pending && self.next_frame.is_event(event).is_some() {
            // The backend compile happens at draw: an error there shows up
            // one frame after the apply.
            self.live_error_pending = false;
            if let Some(err) = makepad_platform::shader_error::take() {
                self.live_revert(cx, &err);
            }
        }
        if self.swatch_refresh && self.next_frame.is_event(event).is_some() {
            self.swatch_refresh = false;
            self.redraw_sidebar(cx);
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
                            VisKind::TweakHeader(..) => {
                                self.tweakables_open = !self.tweakables_open;
                                self.redraw_sidebar(cx);
                            }
                            VisKind::Tweakable(_) | VisKind::InputsHeader(_) | VisKind::CascadeLevel(_) => {}
                            VisKind::Material(mi) => {
                                // Thumbnail click: jump to the Shader tab
                                // with this draw layer loaded.
                                if let Some(layer) = self.materials.get(mi) {
                                    self.vibe_layer = Some(layer.clone());
                                    self.panel_tab = PanelTab::Shader;
                                    self.redraw_sidebar(cx);
                                }
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
                if ke.key_code == KeyCode::ReturnKey
                    && (ke.modifiers.control || ke.modifiers.logo)
                    && tweak_is_on()
                    && self.panel_tab == PanelTab::Shader =>
            {
                let text = self
                    .sidebar
                    .as_ref()
                    .map(|s| {
                        s.child(live_id!(shader_col))
                            .child(live_id!(src_scroll))
                            .child(live_id!(shader_src))
                            .text()
                    })
                    .unwrap_or_default();
                // The prompt box owns Ctrl+Enter when IT has focus (the AI
                // loop); the editor's edit applies otherwise.
                let prompt_focused = self
                    .sidebar
                    .as_ref()
                    .map(|s| cx.has_key_focus(s.child(live_id!(shader_col)).child(live_id!(prompt)).area()))
                    .unwrap_or(false);
                if !prompt_focused && text.contains("fn") {
                    if let Err(error) = apply_fn_edit(cx, self, &text) {
                        self.live_revert(cx, &error);
                    } else {
                        self.live_last_applied = text.clone();
                        self.live_last_good = text;
                    }
                }
            }
            Event::KeyDown(ke) if ke.key_code == KeyCode::Insert && tweak_is_on() => {
                // Insert: a note on the item we are IN — the pinned
                // selection, else the widget under the hover (which becomes
                // the selection so the card has something to ride with).
                // (Ctrl+Space is macOS's input-source switch; the user
                // picked Insert, with the panel's note button as the
                // fallback for keyboards without one.)
                let sel_path = {
                    let mut s = session().lock().unwrap();
                    if s.pinned.is_none() {
                        if let Some(h) = s.hover.clone() {
                            s.pinned = Some(h);
                        }
                    }
                    s.pinned.as_ref().map(|p| p.path.clone())
                };
                if let Some(path) = sel_path {
                    self.note_open = !self.note_open;
                    if self.note_open {
                        let mut s = session().lock().unwrap();
                        if !s.notes.iter().any(|n| n.path == path) {
                            s.notes.push(TweakNote {
                                path,
                                text: String::new(),
                                dx: 8.0,
                                dy: -78.0,
                            });
                        }
                        self.note_seed_pending = true;
                    } else {
                        self.note_rect = None;
                    }
                    self.redraw_overlay(cx);
                }
            }
            _ if self.note_request && tweak_is_on() => {
                self.note_request = false;
                // Insert: a note on the item we are IN — the pinned
                // selection, else the widget under the hover (which becomes
                // the selection so the card has something to ride with).
                // (Ctrl+Space is macOS's input-source switch; the user
                // picked Insert, with the panel's note button as the
                // fallback for keyboards without one.)
                let sel_path = {
                    let mut s = session().lock().unwrap();
                    if s.pinned.is_none() {
                        if let Some(h) = s.hover.clone() {
                            s.pinned = Some(h);
                        }
                    }
                    s.pinned.as_ref().map(|p| p.path.clone())
                };
                if let Some(path) = sel_path {
                    self.note_open = !self.note_open;
                    if self.note_open {
                        let mut s = session().lock().unwrap();
                        if !s.notes.iter().any(|n| n.path == path) {
                            s.notes.push(TweakNote {
                                path,
                                text: String::new(),
                                dx: 8.0,
                                dy: -78.0,
                            });
                        }
                        self.note_seed_pending = true;
                    } else {
                        self.note_rect = None;
                    }
                    self.redraw_overlay(cx);
                }
            }
            Event::KeyDown(ke)
                if ke.key_code == KeyCode::KeyZ
                    && ke.modifiers.logo
                    && tweak_is_on()
                    && cx.key_focus() == Area::Empty =>
            {
                if ke.modifiers.shift {
                    self.redo(cx);
                } else {
                    self.undo(cx);
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
            Event::Scroll(_) if tweak_is_on() => {
                // Content may have moved under the outlines: repaint the
                // overlay with the freshly-read rects.
                self.redraw_overlay(cx);
            }
            Event::MouseMove(e)
                if Some(e.window_id.id()) == self.my_window
                    && !self.splitter_drag
                    && tweak_is_on() =>
            {
                // The doc tooltip: a row whose prop carries doc-channel
                // text shows it, anchored to the row (no per-pixel churn).
                // Tree tab: hovering a row outlines its widget in the body.
                if self.panel_tab == PanelTab::Tree
                    && self.band.size.x > 0.0
                    && e.abs.x > self.band.pos.x
                {
                    let mut hover_target = None;
                    for (item, target) in &self.tree_visible {
                        let rect = item.area().clipped_rect_union(cx);
                        if rect.size.y > 0.0 && rect.contains(e.abs) {
                            hover_target = Some(*target);
                            break;
                        }
                    }
                    match hover_target {
                        Some(target) => {
                            let widget = cx.widget_tree().widget(WidgetUid(target));
                            if !widget.is_empty() {
                                let rect = widget.area().clipped_rect_union(cx);
                                if rect.size.x > 0.0 {
                                    session().lock().unwrap().hover = Some(TweakPick {
                                        uid: target,
                                        path: String::new(),
                                        ty: String::new(),
                                        rect,
                                        window_id: self.my_window.unwrap_or(0),
                                        band: None,
                                        level: 0,
                                    });
                                    self.tree_hover_active = true;
                                    self.redraw_overlay(cx);
                                }
                            }
                        }
                        None => {
                            if self.tree_hover_active {
                                self.tree_hover_active = false;
                                session().lock().unwrap().hover = None;
                                self.redraw_overlay(cx);
                            }
                        }
                    }
                }
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
        // The popover draws above the list but its row is last in event
        // order, so the rows underneath claimed hovers and presses first
        // (first claimant wins). Pointer events inside the popover go to
        // its row before the list; the list's pass then finds them handled.
        let pointer = match event {
            Event::MouseMove(e) => Some(e.abs),
            Event::MouseDown(e) => Some(e.abs),
            Event::MouseUp(e) => Some(e.abs),
            _ => None,
        };
        if let Some(abs) = pointer {
            if self.open_popup.is_some_and(|rect| rect.contains(abs)) {
                let owner = self
                    .visible
                    .iter()
                    .find(|v| {
                        v.item
                            .child(live_id!(swatch))
                            .borrow::<FabColorPick>()
                            .is_some_and(|p| p.is_open())
                    })
                    .map(|v| v.item.clone());
                if let Some(item) = owner {
                    item.handle_event(cx, event, scope);
                }
            }
        }
        if let Some(sidebar) = self.sidebar.clone() {
            if !swallow_scroll {
                sidebar.handle_event(cx, event, scope);
            }
        }
        if self.note_open {
            if let Some(ui) = self.note_ui.clone() {
                ui.handle_event(cx, event, scope);
            }
            match event {
                Event::MouseDown(e) if e.button.is_primary() => {
                    if let Some(rect) = self.note_rect {
                        let grip = Rect {
                            pos: rect.pos,
                            size: dvec2(rect.size.x, 12.0),
                        };
                        if grip.contains(e.abs) {
                            self.note_drag = Some(dvec2(
                                e.abs.x - rect.pos.x,
                                e.abs.y - rect.pos.y,
                            ));
                        }
                    }
                }
                Event::MouseMove(e) => {
                    if let Some(grab) = self.note_drag {
                        let sel = session().lock().unwrap().pinned.clone();
                        if let Some(sel) = sel {
                            let mut s = session().lock().unwrap();
                            if let Some(note) =
                                s.notes.iter_mut().find(|n| n.path == sel.path)
                            {
                                note.dx = e.abs.x - grab.x - sel.rect.pos.x;
                                note.dy = e.abs.y - grab.y - sel.rect.pos.y;
                            }
                            drop(s);
                            self.redraw_overlay(cx);
                        }
                    }
                }
                Event::MouseUp(_) => {
                    self.note_drag = None;
                }
                _ => {}
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
            // Tear the surface down for real: both overlay lists are
            // RETAINED by the window's overlay (a stored sub-list keeps its
            // slot and its last items), so skipping them here left the
            // panel, the outlines and the note card painted after F12.
            // Begin and end them empty so nothing of the mode remains.
            for list in [self.overlay_list.as_mut(), self.sidebar_list.as_mut()]
                .into_iter()
                .flatten()
            {
                list.begin_overlay_reuse(cx);
                let size = cx.current_pass_size();
                cx.begin_root_turtle(size, Layout::flow_down());
                cx.end_pass_sized_turtle();
                list.end(cx);
            }
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
        if self.panel_tab == PanelTab::Theme {
            if self.rows_uid != THEME_ROWS || self.rows_gen != apply_gen {
                self.rebuild_theme_rows(cx);
                self.rows_gen = apply_gen;
                self.swatch_refresh = true;
                self.next_frame = cx.new_next_frame();
            }
        } else if let Some(sel_pick) = &sel {
            if self.rows_uid != sel_pick.uid || self.rows_gen != apply_gen {
                let path = sel_pick.path.clone();
                self.rebuild_rows(cx, sel_pick.uid, &path);
                self.rows_gen = apply_gen;
                self.swatch_refresh = true;
                self.next_frame = cx.new_next_frame();
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

        // Selection/hover rects go STALE when containers scroll: re-read
        // the live widget areas every overlay frame (and write back, so
        // /tweak/state reports where things actually are).
        let pinned = pinned.map(|mut pick| {
            let live = cx.widget_tree().widget(WidgetUid(pick.uid));
            if !live.is_empty() {
                let rect = live_rect(cx, &live);
                if rect.size.x > 0.0 && rect != pick.rect {
                    pick.rect = rect;
                    if let Some(pinned) = session().lock().unwrap().pinned.as_mut() {
                        pinned.rect = rect;
                    }
                } else if rect.size.x <= 0.0 {
                    // Not drawn this frame (another tab is up): the pin
                    // stands, but there is nothing on screen to outline.
                    pick.rect = Rect::default();
                }
            }
            pick
        });
        let hover = hover.map(|mut pick| {
            let live = cx.widget_tree().widget(WidgetUid(pick.uid));
            if !live.is_empty() {
                let rect = live_rect(cx, &live);
                pick.rect = if rect.size.x > 0.0 { rect } else { Rect::default() };
            }
            pick
        });
        // Exploded view: the outlines belong on their widgets' planes inside
        // the body pass, not flat on the window pass — hand them to the
        // pass owner as marks and draw nothing here.
        if cx.sploded_active() {
            let max_level = cx.sploded_max_level();
            let mark = |cx: &mut Cx, pick: &TweakPick| {
                if Some(pick.window_id) != window_id || pick.rect.size.x <= 0.0 {
                    return None;
                }
                let level = cx.sploded_depth_of(pick.uid).unwrap_or(pick.level);
                let _ = max_level;
                Some(makepad_platform::sploded::SplodedMark { rect: pick.rect, level: level as f32 })
            };
            let hover_mark = if quiet { None } else { hover.as_ref().and_then(|p| mark(cx, p)) };
            let pinned_mark = pinned.as_ref().and_then(|p| mark(cx, p));
            cx.sploded_set_marks(hover_mark, pinned_mark);
        }
        // NOT an early return: the overlay list and its root turtle were
        // begun above and are ended below — leaving them open let the
        // window's deferred Fill walk resolve against this turtle instead
        // of its own (an index-out-of-bounds in `resolve_fill`).
        let flat_outlines = !cx.sploded_active();
        if flat_outlines {
        if let Some(pick) = &pinned {
            if Some(pick.window_id) == window_id {
                let style = if quiet {
                    PickStyle::PinnedQuiet
                } else {
                    PickStyle::Pinned
                };
                self.draw_pick(cx, pick, style);
                // Direct-manipulation handles, HOVER-REVEALED: the radius
                // dots exist for the hand, not the eye — parked on the
                // selection's corners they read as chrome and hide the very
                // pixels being judged. They appear when the pointer comes
                // within reach of a corner and vanish with it.
                if self.radius_prop.is_some() {
                    let pointer = session().lock().unwrap().pointer_abs;
                    let near = Self::radius_handle_centers(pick.rect).iter().any(|c| {
                        let dx = pointer.x - c.x;
                        let dy = pointer.y - c.y;
                        dx * dx + dy * dy <= 28.0 * 28.0
                    });
                    if near {
                        self.draw_radius_handles(cx, pick.rect);
                    }
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
        // The Ctrl+Space note card rides the SELECTION's live rect.
        self.note_rect = None;
        if self.note_open {
            if let Some(pick) = &pinned {
                let note = {
                    let s = session().lock().unwrap();
                    s.notes.iter().find(|n| n.path == pick.path).cloned()
                };
                if let Some(note) = note {
                    self.ensure_note_ui(cx);
                    let ui = self.note_ui.as_ref().unwrap().clone();
                    let field = ui.child(live_id!(note_text));
                    self.note_text_uid = field.widget_uid().0;
                    if self.note_seed_pending {
                        field.set_text(cx, &note.text);
                        self.note_seed_pending = false;
                    }
                    let pos = dvec2(
                        (pick.rect.pos.x + note.dx).max(0.0),
                        (pick.rect.pos.y + note.dy).max(0.0),
                    );
                    let mut walk = Walk::fit();
                    walk.abs_pos = Some(pos);
                    walk.width = Size::Fixed(210.0);
                    let _ = ui.draw_walk(cx, scope, walk);
                    let rect = ui.area().rect(cx);
                    if rect.size.x > 0.0 {
                        self.note_rect = Some(rect);
                    }
                }
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
            origin: String::new(),
            siblings: 0,
            scope: "this".to_string(),
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
