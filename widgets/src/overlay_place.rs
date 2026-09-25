//! One placement rule for every anchored popup.
//!
//! Why: four widgets grew four private copies of "hang it off the anchor,
//! flip to the other side when it does not fit, pull it back inboard" —
//! DropToggles, DropSlider, ComboBox and DropDown2 each spelled the same
//! arithmetic their own way, and the popover, tooltip and menu work needs
//! a fifth, sixth and seventh copy. This module is that arithmetic once.
//! It is pure: layout points in, layout points out, no `Cx`, so it runs on
//! the event side (where there is no `Cx2d` to lay anything out in) and
//! every rule has a unit test.
//!
//! The rule, in order:
//! 1. `match_anchor_width` widens the popup to the anchor; it never narrows.
//! 2. An axis whose bounds extent is zero or negative is UNBOUNDED: no flip,
//!    no shift, no clamp on it. That is how a site says "I do not know the
//!    pass yet" (the first event after opening) or "I never flipped and do
//!    not want to start now" — without an `Option` in the request.
//! 3. Main axis (the side's own): keep the requested side when the popup
//!    fits there; else take the opposite side when it fits there; else take
//!    the roomier side, ties going to the requested one. Then shrink the
//!    popup to the room on the chosen side, AWAY from the anchor: the
//!    anchor-facing edge stays put, only the far edge moves.
//! 4. Cross axis: start from the alignment, shrink to the bounds, then pull
//!    inboard with the low edge winning when the popup is still too big.
//! 5. `arrow_at` is where a pointer would sit: on the popup edge that faces
//!    the anchor, under the anchor's centre, clamped to that edge.
//! 6. [`pointer_on_edge`] turns that point into the pointer a popup draws as
//!    part of its own outline, kept off the rounded corners, and
//!    [`slide_for_pointer`] first moves a popup off an anchor too small for
//!    that pointer to reach its middle otherwise.
//!
//! Nothing here panics: the clamps are `max`/`min` pairs, so a request with
//! a negative size or an inside-out rect gives a degenerate answer, not a
//! crash on the event thread.

use crate::makepad_draw::*;

use crate::makepad_draw::{dvec2, DVec2, Rect};

/** Which edge of the anchor the popup hangs off. */
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

impl Side {
    /** The side across the anchor: where a flip lands. */
    pub fn opposite(self) -> Side {
        match self {
            Side::Top => Side::Bottom,
            Side::Bottom => Side::Top,
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }

    /** True for Top and Bottom: the popup hangs along y and aligns along x. */
    pub fn is_vertical(self) -> bool {
        matches!(self, Side::Top | Side::Bottom)
    }
}

/** How the popup lines up with the anchor along the cross axis. Named
`PlaceAlign` because the turtle's `Align` is glob-exported into every widget
file and a second `Align` would make each of those uses ambiguous. */
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceAlign {
    /** The popup's low edge on the anchor's low edge (left edges, or top edges). */
    Start,
    /** The popup centred on the anchor. */
    Center,
    /** The popup's high edge on the anchor's high edge (right edges, or bottom edges). */
    End,
}

/// Which event last had its Escape claimed by an overlay.
#[derive(Default)]
struct EscapeClaim {
    event_id: u64,
}

/// Claim this Escape for one overlay, and answer whether the claim was
/// won.
///
/// Overlays cannot decide this from the lock stack. They are dispatched in
/// tree order, not innermost-first, and the first one to act releases its
/// lock as it closes; the next one down then looks, sees nothing above it
/// and closes too, so one press unwinds the whole stack. A claim keyed to
/// the event settles it: the first overlay to ask gets the press and every
/// other one leaves it alone until the next.
///
/// The dispatch order decides who asks first, so an overlay should still
/// check that nothing is locked above it before asking — the claim stops
/// the second closer, and the lock check stops the wrong one from being
/// first.
pub fn claim_escape(cx: &mut Cx) -> bool {
    let event_id = cx.event_id();
    let claim = cx.global::<EscapeClaim>();
    if claim.event_id == event_id {
        return false;
    }
    claim.event_id = event_id;
    true
}

thread_local! {
    /// Sweep locks whose owners were dropped still holding them. A widget is
    /// dropped where no `Cx` is at hand to release anything, so the areas
    /// wait here for the next event.
    static ORPHANED_LOCKS: std::cell::RefCell<Vec<Area>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Leave sweep locks for the next event to release, from a `Drop` that has
/// no `Cx`. An overlay dropped while it held the pointer (its page rebuilt
/// on a theme or story switch while it was open) would otherwise turn away
/// every hit test in the window for good.
pub(crate) fn orphan_sweep_locks(areas: &[Area]) {
    let _ = ORPHANED_LOCKS.try_with(|orphans| orphans.borrow_mut().extend_from_slice(areas));
}

/// Release every lock a dropped overlay left behind: the ones it handed
/// over from its `Drop`, and the ones held by an owner that had no `Drop` to
/// hand them over with (see [`release_stale_sweep_locks`]). The window does
/// this as each event reaches it, so a tree left with no overlay of the kind
/// that was dropped still gets its input back; an overlay does it too before
/// taking a lock of its own, for a tree with no window above it.
/// Forget the last Escape claim and every orphaned lock: a pooled test
/// context is handed to case after case without an event loop, so its
/// event id never moves and the first case's claim would refuse every
/// later case's Escape.
#[cfg(test)]
pub(crate) fn reset_for_test(cx: &mut Cx) {
    // `set_global` keeps an existing global; this one must be replaced.
    *cx.global::<EscapeClaim>() = EscapeClaim::default();
    let _ = ORPHANED_LOCKS.try_with(|orphans| orphans.borrow_mut().clear());
}
pub(crate) fn release_orphaned_sweep_locks(cx: &mut Cx) {
    let orphans = ORPHANED_LOCKS
        .try_with(|orphans| std::mem::take(&mut *orphans.borrow_mut()))
        .unwrap_or_default();
    for area in orphans {
        cx.sweep_unlock(area);
    }
    release_stale_sweep_locks(cx);
}

/// Let go of every lock whose area names nothing drawn any more: its draw
/// list was dropped along with the page it belonged to, or has been drawn
/// again without the owner drawing into it. An owner that is still drawn
/// never reads as gone, because each draw moves the lock to the fresh handle
/// together with the owner's own area.
///
/// Why here and not only in each overlay's `Drop`: most widgets that take
/// the lock have no `Drop`, and one dropped open (its page rebuilt on a story
/// or theme switch) turned away every press in the window until the app was
/// restarted. This catches all of them, once the list is drawn again or
/// freed; a `Drop` that orphans its lock only lets go a frame sooner.
///
/// The stack is taken apart and the living owners put back in their order,
/// so a stale lock under a living one goes too and the nesting is kept.
fn release_stale_sweep_locks(cx: &mut Cx) {
    if cx.sweep_lock_area().is_none() {
        return;
    }
    let mut held = Vec::new();
    while let Some(top) = cx.sweep_lock_area() {
        held.push(top);
        cx.sweep_unlock(top);
    }
    for area in held.into_iter().rev() {
        if !names_nothing_drawn(cx, area) {
            cx.sweep_lock(area);
        }
    }
}

/// True when an area's draw list has been freed, or drawn again since the
/// area was handed out. An empty area names no list, so it is never judged
/// here.
fn names_nothing_drawn(cx: &Cx, area: Area) -> bool {
    match area.draw_list_id() {
        Some(list) => cx.draw_lists.is_id_freed(list) || !area.is_valid(cx),
        None => false,
    }
}

/** A side and an alignment: one of the twelve places a popup can hang. */
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub side: Side,
    pub align: PlaceAlign,
}

impl Placement {
    pub const fn new(side: Side, align: PlaceAlign) -> Placement {
        Placement { side, align }
    }

    pub const TOP_START: Placement = Placement::new(Side::Top, PlaceAlign::Start);
    pub const TOP_CENTER: Placement = Placement::new(Side::Top, PlaceAlign::Center);
    pub const TOP_END: Placement = Placement::new(Side::Top, PlaceAlign::End);
    pub const BOTTOM_START: Placement = Placement::new(Side::Bottom, PlaceAlign::Start);
    pub const BOTTOM_CENTER: Placement = Placement::new(Side::Bottom, PlaceAlign::Center);
    pub const BOTTOM_END: Placement = Placement::new(Side::Bottom, PlaceAlign::End);
    pub const LEFT_START: Placement = Placement::new(Side::Left, PlaceAlign::Start);
    pub const LEFT_CENTER: Placement = Placement::new(Side::Left, PlaceAlign::Center);
    pub const LEFT_END: Placement = Placement::new(Side::Left, PlaceAlign::End);
    pub const RIGHT_START: Placement = Placement::new(Side::Right, PlaceAlign::Start);
    pub const RIGHT_CENTER: Placement = Placement::new(Side::Right, PlaceAlign::Center);
    pub const RIGHT_END: Placement = Placement::new(Side::Right, PlaceAlign::End);
}

/** Everything `place` needs, all in one coordinate space (window-absolute
layout points at every current site). */
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaceRequest {
    /** The rect the popup hangs off. */
    pub anchor: Rect,
    /** The popup's wanted size, before any widening to the anchor or
    shrinking to the bounds. */
    pub size: DVec2,
    /** Where the popup may go. An axis whose extent is zero or negative is
    unbounded on that axis: no flip, no shift, no shrink there. */
    pub bounds: Rect,
    /** Distance between the anchor's edge and the popup's facing edge. */
    pub gap: f64,
    /** The wanted side and alignment; `Placed::side` says which side was
    actually taken. */
    pub placement: Placement,
    /** Widen the popup to the anchor's width when it is narrower; it is
    never narrowed. Pass a zero width to get exactly the anchor's. This is
    the `max(content, trigger)` rule both list popups use. */
    pub match_anchor_width: bool,
}

/** What `place` decided. */
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    /** Where the popup goes, after the flip, the shift and the shrink. */
    pub rect: Rect,
    /** The side taken; differs from the request's when it flipped. */
    pub side: Side,
    /** The point on the popup's anchor-facing edge under the anchor's
    centre, clamped to that edge, for a pointer or arrow to sit at. */
    pub arrow_at: DVec2,
}

/// Pull a span of `extent` back inside `[lo, lo + room]`: the start is
/// clamped so the span ends before `lo + room`, and when the span is longer
/// than the room the low edge wins. A `room` of zero or less means
/// unbounded and returns `start` untouched. Never panics.
pub fn span_inboard(start: f64, extent: f64, lo: f64, room: f64) -> f64 {
    if room > 0.0 {
        start.max(lo).min((lo + room - extent).max(lo))
    } else {
        start
    }
}

/// Decide where a popup of `req.size` hangs off `req.anchor`: flip when it
/// does not fit, shift it inboard, shrink it to the room. See the module
/// doc for the rule.
pub fn place_overlay(req: &PlaceRequest) -> Placed {
    let anchor = req.anchor;
    let bounds = req.bounds;
    let gap = req.gap;
    let mut size = req.size;
    if req.match_anchor_width {
        size.x = size.x.max(anchor.size.x);
    }
    let requested = req.placement.side;
    let vertical = requested.is_vertical();

    // Main axis: the side's own. Room is the distance from the anchor's
    // edge plus the gap to the bounds' edge on that side, NOT floored, so
    // "does not fit" and "roomier" both read straight off it.
    let room = |side: Side| -> f64 {
        match side {
            Side::Bottom => (bounds.pos.y + bounds.size.y) - (anchor.pos.y + anchor.size.y + gap),
            Side::Top => (anchor.pos.y - gap) - bounds.pos.y,
            Side::Right => (bounds.pos.x + bounds.size.x) - (anchor.pos.x + anchor.size.x + gap),
            Side::Left => (anchor.pos.x - gap) - bounds.pos.x,
        }
    };
    let (main_extent, main_bound) = if vertical {
        (size.y, bounds.size.y)
    } else {
        (size.x, bounds.size.x)
    };
    let (side, main_extent) = if main_bound > 0.0 {
        let opposite = requested.opposite();
        let side = if main_extent <= room(requested) {
            requested
        } else if main_extent <= room(opposite) {
            opposite
        } else if room(opposite) > room(requested) {
            opposite
        } else {
            requested
        };
        (side, main_extent.min(room(side).max(0.0)))
    } else {
        (requested, main_extent)
    };

    // Cross axis: align, shrink to the bounds, pull inboard.
    let (anchor_start, anchor_extent, cross_extent, bounds_start, bounds_extent) = if vertical {
        (anchor.pos.x, anchor.size.x, size.x, bounds.pos.x, bounds.size.x)
    } else {
        (anchor.pos.y, anchor.size.y, size.y, bounds.pos.y, bounds.size.y)
    };
    let cross_extent = if bounds_extent > 0.0 {
        cross_extent.min(bounds_extent)
    } else {
        cross_extent
    };
    let cross_start = match req.placement.align {
        PlaceAlign::Start => anchor_start,
        PlaceAlign::Center => anchor_start + (anchor_extent - cross_extent) * 0.5,
        PlaceAlign::End => anchor_start + anchor_extent - cross_extent,
    };
    let cross_start = span_inboard(cross_start, cross_extent, bounds_start, bounds_extent);

    let rect = match side {
        Side::Bottom => Rect {
            pos: dvec2(cross_start, anchor.pos.y + anchor.size.y + gap),
            size: dvec2(cross_extent, main_extent),
        },
        Side::Top => Rect {
            pos: dvec2(cross_start, anchor.pos.y - gap - main_extent),
            size: dvec2(cross_extent, main_extent),
        },
        Side::Right => Rect {
            pos: dvec2(anchor.pos.x + anchor.size.x + gap, cross_start),
            size: dvec2(main_extent, cross_extent),
        },
        Side::Left => Rect {
            pos: dvec2(anchor.pos.x - gap - main_extent, cross_start),
            size: dvec2(main_extent, cross_extent),
        },
    };

    let centre = anchor.center();
    let along_x = centre.x.max(rect.pos.x).min(rect.pos.x + rect.size.x);
    let along_y = centre.y.max(rect.pos.y).min(rect.pos.y + rect.size.y);
    let arrow_at = match side {
        Side::Bottom => dvec2(along_x, rect.pos.y),
        Side::Top => dvec2(along_x, rect.pos.y + rect.size.y),
        Side::Right => dvec2(rect.pos.x, along_y),
        Side::Left => dvec2(rect.pos.x + rect.size.x, along_y),
    };

    Placed {
        rect,
        side,
        arrow_at,
    }
}

/** A pointer drawn as part of a popup's own outline, in the popup's local
space: the popup's top-left corner is the origin. */
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pointer {
    /** The middle of the pointer's base, on the popup's outline. */
    pub base: DVec2,
    /** The point, out from the base toward the anchor. */
    pub tip: DVec2,
}

/// Put a pointer on the edge of a popup of `size` that faces its anchor.
///
/// `side` is the side of the anchor the popup was placed on
/// ([`Placed::side`]), so the pointer is on the popup's opposite edge,
/// aiming back. `at` is the point to aim at in the popup's local space —
/// [`Placed::arrow_at`] less the placed rect's position — and only its
/// coordinate along the edge is read, so the pointer slides along the edge
/// to follow an anchor the popup was shifted away from. `length` is how far
/// the point stands out; the base is twice that. `outline` is how far inside
/// the popup's rect its outline runs, which is where the base goes. `corner`
/// is how far a rounded corner reaches along the edge from each end: the
/// whole base stays on the straight part between them, because a base that
/// straddles a corner leaves a notch where the curve falls away under it.
/// An edge too short for that holds the pointer at its middle.
pub fn pointer_on_edge(side: Side, size: DVec2, at: DVec2, length: f64, outline: f64, corner: f64) -> Pointer {
    let length = length.max(0.0);
    let along = |v: f64, extent: f64| -> f64 {
        let lo = corner + length;
        let hi = extent - corner - length;
        if hi < lo {
            extent * 0.5
        } else {
            v.max(lo).min(hi)
        }
    };
    match side {
        // The popup is below the anchor: the pointer is on its top edge.
        Side::Bottom => {
            let x = along(at.x, size.x);
            Pointer {
                base: dvec2(x, outline),
                tip: dvec2(x, outline - length),
            }
        }
        Side::Top => {
            let x = along(at.x, size.x);
            Pointer {
                base: dvec2(x, size.y - outline),
                tip: dvec2(x, size.y - outline + length),
            }
        }
        // The popup is right of the anchor: the pointer is on its left edge.
        Side::Right => {
            let y = along(at.y, size.y);
            Pointer {
                base: dvec2(outline, y),
                tip: dvec2(outline - length, y),
            }
        }
        Side::Left => {
            let y = along(at.y, size.y);
            Pointer {
                base: dvec2(size.x - outline, y),
                tip: dvec2(size.x - outline + length, y),
            }
        }
    }
}

/// Slide a placed popup along the edge that faces its anchor, just far
/// enough that a pointer kept `keep` in from each end of that edge (the
/// `corner` plus the `length` given to [`pointer_on_edge`]) lands on the
/// anchor's middle.
///
/// Only an anchor shorter than twice `keep` along that edge moves the popup,
/// down to the one-point anchor a press at the pointer gives. Lined up with
/// such an anchor's start or end, the popup puts the anchor's middle on the
/// stretch of edge a pointer may not take, and the pointer would stop past
/// that middle, or past the anchor altogether. A longer anchor leaves the
/// popup where the placement put it: its middle is already within reach,
/// or, when the anchor is longer than the popup, the slide it would take is
/// as long as the anchor and would undo the alignment that was asked for,
/// so the pointer takes the nearest end of the straight part, which is
/// still over the anchor.
///
/// The slide stops at `bounds` and never pulls back a popup that is already
/// past them; an axis with no extent is unbounded, as it is for
/// [`place_overlay`]. `arrow_at` moves with the popup.
pub fn slide_for_pointer(placed: Placed, anchor: Rect, bounds: Rect, keep: f64) -> Placed {
    let vertical = placed.side.is_vertical();
    let along = |v: DVec2| if vertical { v.x } else { v.y };
    if along(anchor.size) >= keep * 2.0 {
        return placed;
    }
    let (start, extent) = (along(placed.rect.pos), along(placed.rect.size));
    let centre = along(anchor.center());
    // Where on the edge the pointer can take the anchor's middle, by the
    // rule `pointer_on_edge` places it with: the nearest point of the
    // straight part, or the middle of an edge too short to have one.
    let reach = if extent < keep * 2.0 {
        extent * 0.5
    } else {
        (centre - start).max(keep).min(extent - keep)
    };
    let wanted = centre - reach;
    let (lo, room) = (along(bounds.pos), along(bounds.size));
    let moved = if room <= 0.0 {
        wanted
    } else if wanted < start {
        wanted.max(start.min(lo))
    } else {
        wanted.min(start.max(lo + room - extent))
    };
    let mut rect = placed.rect;
    let mut arrow_at = placed.arrow_at;
    let point = centre.max(moved).min(moved + extent);
    if vertical {
        rect.pos.x = moved;
        arrow_at.x = point;
    } else {
        rect.pos.y = moved;
        arrow_at.y = point;
    }
    Placed { rect, side: placed.side, arrow_at }
}

#[cfg(test)]
mod tests {
    /// The pooled test contexts are handed from case to case with one
    /// event id between them: the last case's claim must not refuse the
    /// next case's Escape.
    #[test]
    fn a_pooled_context_forgets_the_last_claim() {
        crate::on_test_cx(|| {
            {
                let mut cx = crate::checkout_test_cx();
                assert!(super::claim_escape(&mut cx), "a fresh context has no claim");
                assert!(!super::claim_escape(&mut cx), "one event is claimed once");
            }
            let mut cx = crate::checkout_test_cx();
            assert!(super::claim_escape(&mut cx), "the next case is not refused by the last one's claim");
        });
    }

    use super::*;

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect {
            pos: dvec2(x, y),
            size: dvec2(w, h),
        }
    }

    /// A 100x20 anchor at (200,200) in an 800x600 window, a 60x40 popup,
    /// gap 4, hanging below with left edges aligned. Tests override fields.
    fn base() -> PlaceRequest {
        PlaceRequest {
            anchor: r(200.0, 200.0, 100.0, 20.0),
            size: dvec2(60.0, 40.0),
            bounds: r(0.0, 0.0, 800.0, 600.0),
            gap: 4.0,
            placement: Placement::BOTTOM_START,
            match_anchor_width: false,
        }
    }

    // -- sides and alignment ---------------------------------------------

    #[test]
    fn sides_know_their_opposite_and_axis() {
        crate::on_test_cx(|| {
        assert_eq!(Side::Top.opposite(), Side::Bottom);
        assert_eq!(Side::Bottom.opposite(), Side::Top);
        assert_eq!(Side::Left.opposite(), Side::Right);
        assert_eq!(Side::Right.opposite(), Side::Left);
        assert!(Side::Top.is_vertical());
        assert!(Side::Bottom.is_vertical());
        assert!(!Side::Left.is_vertical());
        assert!(!Side::Right.is_vertical());
        });
    }

    #[test]
    fn fits_below_and_stays_there() {
        crate::on_test_cx(|| {
        let p = place_overlay(&base());
        assert_eq!(p.rect, r(200.0, 224.0, 60.0, 40.0));
        assert_eq!(p.side, Side::Bottom);
        assert_eq!(p.arrow_at, dvec2(250.0, 224.0));
        });
    }

    #[test]
    fn fits_exactly_and_still_stays() {
        crate::on_test_cx(|| {
        // Room below is exactly the popup's height: 600 - (556 + 4) = 40.
        let p = place_overlay(&PlaceRequest {
            anchor: r(200.0, 536.0, 100.0, 20.0),
            ..base()
        });
        assert_eq!(p.side, Side::Bottom);
        assert_eq!(p.rect, r(200.0, 560.0, 60.0, 40.0));
        });
    }

    #[test]
    fn each_of_the_twelve_placements_lands_where_it_says() {
        crate::on_test_cx(|| {
        // (placement, expected rect, expected arrow) around the base anchor.
        let table = [
            (Placement::BOTTOM_START, r(200.0, 224.0, 60.0, 40.0), dvec2(250.0, 224.0)),
            (Placement::BOTTOM_CENTER, r(220.0, 224.0, 60.0, 40.0), dvec2(250.0, 224.0)),
            (Placement::BOTTOM_END, r(240.0, 224.0, 60.0, 40.0), dvec2(250.0, 224.0)),
            (Placement::TOP_START, r(200.0, 156.0, 60.0, 40.0), dvec2(250.0, 196.0)),
            (Placement::TOP_CENTER, r(220.0, 156.0, 60.0, 40.0), dvec2(250.0, 196.0)),
            (Placement::TOP_END, r(240.0, 156.0, 60.0, 40.0), dvec2(250.0, 196.0)),
            (Placement::RIGHT_START, r(304.0, 200.0, 60.0, 40.0), dvec2(304.0, 210.0)),
            (Placement::RIGHT_CENTER, r(304.0, 190.0, 60.0, 40.0), dvec2(304.0, 210.0)),
            (Placement::RIGHT_END, r(304.0, 180.0, 60.0, 40.0), dvec2(304.0, 210.0)),
            (Placement::LEFT_START, r(136.0, 200.0, 60.0, 40.0), dvec2(196.0, 210.0)),
            (Placement::LEFT_CENTER, r(136.0, 190.0, 60.0, 40.0), dvec2(196.0, 210.0)),
            (Placement::LEFT_END, r(136.0, 180.0, 60.0, 40.0), dvec2(196.0, 210.0)),
        ];
        for (placement, rect, arrow) in table {
            let p = place_overlay(&PlaceRequest {
                placement,
                ..base()
            });
            assert_eq!(p.side, placement.side, "{placement:?}");
            assert_eq!(p.rect, rect, "{placement:?}");
            assert_eq!(p.arrow_at, arrow, "{placement:?}");
        }
        });
    }

    // -- flipping ----------------------------------------------------------

    #[test]
    fn flips_up_when_below_is_short_and_above_fits() {
        crate::on_test_cx(|| {
        // Room below: 600 - (590 + 4) = 6; room above: 566.
        let p = place_overlay(&PlaceRequest {
            anchor: r(200.0, 570.0, 100.0, 20.0),
            ..base()
        });
        assert_eq!(p.side, Side::Top);
        assert_eq!(p.rect, r(200.0, 526.0, 60.0, 40.0));
        assert_eq!(p.arrow_at, dvec2(250.0, 566.0));
        });
    }

    #[test]
    fn flips_left_when_right_is_short_and_left_fits() {
        crate::on_test_cx(|| {
        // Room right: 800 - (780 + 4) = 16; room left: 696.
        let p = place_overlay(&PlaceRequest {
            anchor: r(700.0, 200.0, 80.0, 20.0),
            placement: Placement::RIGHT_START,
            ..base()
        });
        assert_eq!(p.side, Side::Left);
        assert_eq!(p.rect, r(636.0, 200.0, 60.0, 40.0));
        assert_eq!(p.arrow_at, dvec2(696.0, 210.0));
        });
    }

    #[test]
    fn neither_fits_takes_the_roomier_side_and_shrinks_to_it() {
        crate::on_test_cx(|| {
        // A 100-tall window, anchor at y=30: room below 100 - 54 = 46, room
        // above 30 - 4 = 26. A 60-tall popup fits neither; below wins on
        // room and the popup is cut to 46 with its top edge still on the
        // anchor's bottom plus the gap.
        let p = place_overlay(&PlaceRequest {
            anchor: r(200.0, 30.0, 100.0, 20.0),
            size: dvec2(60.0, 60.0),
            bounds: r(0.0, 0.0, 800.0, 100.0),
            ..base()
        });
        assert_eq!(p.side, Side::Bottom);
        assert_eq!(p.rect, r(200.0, 54.0, 60.0, 46.0));
        // Anchor at y=46: room below 100 - 70 = 30, room above 42; the same
        // 60-tall popup flips and is cut to 42, its bottom edge on the
        // anchor's top minus the gap.
        let p = place_overlay(&PlaceRequest {
            anchor: r(200.0, 46.0, 100.0, 20.0),
            size: dvec2(60.0, 60.0),
            bounds: r(0.0, 0.0, 800.0, 100.0),
            ..base()
        });
        assert_eq!(p.side, Side::Top);
        assert_eq!(p.rect, r(200.0, 0.0, 60.0, 42.0));
        assert_eq!(p.rect.pos.y + p.rect.size.y, 42.0);
        });
    }

    #[test]
    fn a_tie_between_two_short_sides_keeps_the_requested_side() {
        crate::on_test_cx(|| {
        // Room below is pass - y - h - gap and room above is y - gap; they
        // tie when y = (pass - h) / 2. In a 100-tall window with a 20-tall
        // anchor that is y = 40: both rooms are 36, short of the 40 asked.
        let p = place_overlay(&PlaceRequest {
            anchor: r(200.0, 40.0, 100.0, 20.0),
            bounds: r(0.0, 0.0, 800.0, 100.0),
            ..base()
        });
        assert_eq!(p.side, Side::Bottom, "{p:?}");
        assert_eq!(p.rect.size.y, 36.0);
        let p = place_overlay(&PlaceRequest {
            anchor: r(200.0, 40.0, 100.0, 20.0),
            bounds: r(0.0, 0.0, 800.0, 100.0),
            placement: Placement::TOP_START,
            ..base()
        });
        assert_eq!(p.side, Side::Top, "{p:?}");
        assert_eq!(p.rect.size.y, 36.0);
        });
    }

    #[test]
    fn the_requested_side_wins_when_it_is_roomier_even_though_nothing_fits() {
        crate::on_test_cx(|| {
        // Room below 376, room above 196, popup 1000 tall: no flip, 376 tall.
        let p = place_overlay(&PlaceRequest {
            size: dvec2(60.0, 1000.0),
            ..base()
        });
        assert_eq!(p.side, Side::Bottom);
        assert_eq!(p.rect, r(200.0, 224.0, 60.0, 376.0));
        assert_eq!(p.arrow_at, dvec2(250.0, 224.0));
        });
    }

    #[test]
    fn taller_than_the_window_flips_when_above_is_roomier() {
        crate::on_test_cx(|| {
        // Anchor at y=500: room below 76, room above 496.
        let p = place_overlay(&PlaceRequest {
            anchor: r(200.0, 500.0, 100.0, 20.0),
            size: dvec2(60.0, 1000.0),
            ..base()
        });
        assert_eq!(p.side, Side::Top);
        assert_eq!(p.rect, r(200.0, 0.0, 60.0, 496.0));
        assert_eq!(p.arrow_at, dvec2(250.0, 496.0));
        });
    }

    #[test]
    fn no_room_on_either_side_gives_a_zero_height_not_a_negative_one() {
        crate::on_test_cx(|| {
        // Anchor fills the whole window: both rooms are negative.
        let p = place_overlay(&PlaceRequest {
            anchor: r(0.0, 0.0, 800.0, 600.0),
            ..base()
        });
        assert_eq!(p.rect.size.y, 0.0);
        assert!(p.rect.size.y >= 0.0);
        });
    }

    // -- shifting ----------------------------------------------------------

    #[test]
    fn shifts_inboard_on_the_right_edge() {
        crate::on_test_cx(|| {
        let p = place_overlay(&PlaceRequest {
            anchor: r(770.0, 200.0, 100.0, 20.0),
            ..base()
        });
        assert_eq!(p.rect, r(740.0, 224.0, 60.0, 40.0));
        assert_eq!(p.side, Side::Bottom);
        });
    }

    #[test]
    fn shifts_inboard_on_the_left_edge() {
        crate::on_test_cx(|| {
        let p = place_overlay(&PlaceRequest {
            anchor: r(-30.0, 200.0, 100.0, 20.0),
            ..base()
        });
        assert_eq!(p.rect, r(0.0, 224.0, 60.0, 40.0));
        });
    }

    #[test]
    fn the_low_edge_wins_when_wider_than_the_bounds() {
        crate::on_test_cx(|| {
        let p = place_overlay(&PlaceRequest {
            bounds: r(10.0, 0.0, 50.0, 600.0),
            ..base()
        });
        assert_eq!(p.rect, r(10.0, 224.0, 50.0, 40.0));
        });
    }

    #[test]
    fn end_alignment_shifts_inboard_too() {
        crate::on_test_cx(|| {
        // End puts the popup's right edge on the anchor's, at 870: past 800.
        let p = place_overlay(&PlaceRequest {
            anchor: r(770.0, 200.0, 100.0, 20.0),
            placement: Placement::BOTTOM_END,
            ..base()
        });
        assert_eq!(p.rect, r(740.0, 224.0, 60.0, 40.0));
        });
    }

    #[test]
    fn horizontal_sides_shift_along_y() {
        crate::on_test_cx(|| {
        let p = place_overlay(&PlaceRequest {
            anchor: r(200.0, 590.0, 100.0, 20.0),
            placement: Placement::RIGHT_START,
            ..base()
        });
        assert_eq!(p.side, Side::Right);
        assert_eq!(p.rect, r(304.0, 560.0, 60.0, 40.0));
        });
    }

    #[test]
    fn arrow_at_follows_the_anchor_centre_and_clamps_to_the_popup_edge() {
        crate::on_test_cx(|| {
        // Shifted 30 to the left: the anchor's centre (820) is off the popup,
        // so the arrow sits at the popup's right corner.
        let p = place_overlay(&PlaceRequest {
            anchor: r(770.0, 200.0, 100.0, 20.0),
            ..base()
        });
        assert_eq!(p.arrow_at, dvec2(800.0, 224.0));
        // A narrow anchor under a wide popup: the arrow is inside the edge.
        let p = place_overlay(&PlaceRequest {
            anchor: r(400.0, 200.0, 10.0, 20.0),
            size: dvec2(300.0, 40.0),
            placement: Placement::BOTTOM_CENTER,
            ..base()
        });
        assert_eq!(p.rect, r(255.0, 224.0, 300.0, 40.0));
        assert_eq!(p.arrow_at, dvec2(405.0, 224.0));
        });
    }

    // -- bounds ------------------------------------------------------------

    #[test]
    fn an_unbounded_request_never_flips_shifts_or_shrinks() {
        crate::on_test_cx(|| {
        let p = place_overlay(&PlaceRequest {
            anchor: r(770.0, 580.0, 100.0, 20.0),
            size: dvec2(60.0, 1000.0),
            bounds: Rect::default(),
            ..base()
        });
        assert_eq!(p.side, Side::Bottom);
        assert_eq!(p.rect, r(770.0, 604.0, 60.0, 1000.0));
        });
    }

    #[test]
    fn a_negative_bounds_extent_is_unbounded_too() {
        crate::on_test_cx(|| {
        let p = place_overlay(&PlaceRequest {
            anchor: r(770.0, 580.0, 100.0, 20.0),
            bounds: r(6.0, 6.0, -12.0, -12.0),
            ..base()
        });
        assert_eq!(p.rect, r(770.0, 604.0, 60.0, 40.0));
        });
    }

    #[test]
    fn each_axis_is_bounded_on_its_own() {
        crate::on_test_cx(|| {
        // Bounded in x only: the popup shifts inboard but does not flip.
        let p = place_overlay(&PlaceRequest {
            anchor: r(770.0, 580.0, 100.0, 20.0),
            bounds: r(0.0, 0.0, 800.0, 0.0),
            ..base()
        });
        assert_eq!(p.side, Side::Bottom);
        assert_eq!(p.rect, r(740.0, 604.0, 60.0, 40.0));
        // Bounded in y only: it flips but keeps its x.
        let p = place_overlay(&PlaceRequest {
            anchor: r(770.0, 580.0, 100.0, 20.0),
            bounds: r(0.0, 0.0, 0.0, 600.0),
            ..base()
        });
        assert_eq!(p.side, Side::Top);
        assert_eq!(p.rect, r(770.0, 536.0, 60.0, 40.0));
        });
    }

    #[test]
    fn bounds_need_not_start_at_the_origin() {
        crate::on_test_cx(|| {
        // A 6-point inset on every edge, like DropToggles keeps.
        let p = place_overlay(&PlaceRequest {
            anchor: r(760.0, 200.0, 100.0, 20.0),
            bounds: r(6.0, 6.0, 788.0, 588.0),
            ..base()
        });
        assert_eq!(p.rect, r(734.0, 224.0, 60.0, 40.0));
        let p = place_overlay(&PlaceRequest {
            anchor: r(-40.0, 200.0, 100.0, 20.0),
            bounds: r(6.0, 6.0, 788.0, 588.0),
            ..base()
        });
        assert_eq!(p.rect.pos.x, 6.0);
        });
    }

    // -- match_anchor_width ------------------------------------------------

    #[test]
    fn match_anchor_width_grows_to_the_anchor_and_never_shrinks() {
        crate::on_test_cx(|| {
        let p = place_overlay(&PlaceRequest {
            match_anchor_width: true,
            ..base()
        });
        assert_eq!(p.rect.size.x, 100.0);
        let p = place_overlay(&PlaceRequest {
            size: dvec2(150.0, 40.0),
            match_anchor_width: true,
            ..base()
        });
        assert_eq!(p.rect.size.x, 150.0);
        let p = place_overlay(&PlaceRequest {
            size: dvec2(0.0, 40.0),
            match_anchor_width: true,
            ..base()
        });
        assert_eq!(p.rect.size.x, 100.0);
        });
    }

    #[test]
    fn a_matched_width_still_shrinks_to_the_bounds() {
        crate::on_test_cx(|| {
        let p = place_overlay(&PlaceRequest {
            anchor: r(0.0, 200.0, 1000.0, 20.0),
            match_anchor_width: true,
            ..base()
        });
        assert_eq!(p.rect, r(0.0, 224.0, 800.0, 40.0));
        });
    }

    // -- span_inboard --------------------------------------------------------

    #[test]
    fn span_inboard_leaves_a_span_that_fits_alone() {
        crate::on_test_cx(|| {
        assert_eq!(span_inboard(100.0, 50.0, 8.0, 784.0), 100.0);
        assert_eq!(span_inboard(8.0, 50.0, 8.0, 784.0), 8.0);
        assert_eq!(span_inboard(742.0, 50.0, 8.0, 784.0), 742.0);
        });
    }

    #[test]
    fn span_inboard_pulls_back_from_the_high_edge() {
        crate::on_test_cx(|| {
        assert_eq!(span_inboard(780.0, 50.0, 8.0, 784.0), 742.0);
        });
    }

    #[test]
    fn span_inboard_pulls_forward_from_the_low_edge() {
        crate::on_test_cx(|| {
        assert_eq!(span_inboard(-20.0, 50.0, 8.0, 784.0), 8.0);
        });
    }

    #[test]
    fn span_inboard_lets_the_low_edge_win_when_too_long() {
        crate::on_test_cx(|| {
        assert_eq!(span_inboard(300.0, 900.0, 8.0, 784.0), 8.0);
        assert_eq!(span_inboard(-300.0, 900.0, 8.0, 784.0), 8.0);
        });
    }

    #[test]
    fn span_inboard_is_a_no_op_without_room() {
        crate::on_test_cx(|| {
        assert_eq!(span_inboard(300.0, 50.0, 8.0, 0.0), 300.0);
        assert_eq!(span_inboard(-300.0, 50.0, 8.0, -12.0), -300.0);
        });
    }

    #[test]
    fn span_inboard_is_the_one_line_the_sites_had() {
        crate::on_test_cx(|| {
        // ComboBox and DropDown2 wrote `x.clamp(m, (pass - m - w).max(m))`.
        let cases: [(f64, f64, f64); 3] = [(360.0, 200.0, 400.0), (-5.0, 30.0, 400.0), (50.0, 500.0, 400.0)];
        for (x, w, pass) in cases {
            let m = 8.0_f64;
            let old = x.clamp(m, (pass - m - w).max(m));
            assert_eq!(span_inboard(x, w, m, pass - m * 2.0), old, "x={x} w={w}");
        }
        });
    }

    // -- pointer_on_edge -----------------------------------------------------

    #[test]
    fn a_pointer_stands_out_of_the_edge_that_faces_the_anchor() {
        crate::on_test_cx(|| {
        let size = dvec2(100.0, 60.0);
        let at = dvec2(50.0, 30.0);
        let below = pointer_on_edge(Side::Bottom, size, at, 7.0, 1.0, 10.0);
        assert_eq!(below, Pointer { base: dvec2(50.0, 1.0), tip: dvec2(50.0, -6.0) });
        let above = pointer_on_edge(Side::Top, size, at, 7.0, 1.0, 10.0);
        assert_eq!(above, Pointer { base: dvec2(50.0, 59.0), tip: dvec2(50.0, 66.0) });
        let right = pointer_on_edge(Side::Right, size, at, 7.0, 1.0, 10.0);
        assert_eq!(right, Pointer { base: dvec2(1.0, 30.0), tip: dvec2(-6.0, 30.0) });
        let left = pointer_on_edge(Side::Left, size, at, 7.0, 1.0, 10.0);
        assert_eq!(left, Pointer { base: dvec2(99.0, 30.0), tip: dvec2(106.0, 30.0) });
        });
    }

    #[test]
    fn a_pointer_keeps_its_whole_base_off_the_corners() {
        crate::on_test_cx(|| {
        let size = dvec2(100.0, 60.0);
        // Aimed past either end: the base's near end stops where the corner does.
        let low = pointer_on_edge(Side::Bottom, size, dvec2(-40.0, 0.0), 7.0, 1.0, 10.0);
        assert_eq!(low.tip.x, 17.0);
        let high = pointer_on_edge(Side::Bottom, size, dvec2(400.0, 0.0), 7.0, 1.0, 10.0);
        assert_eq!(high.tip.x, 83.0);
        let side = pointer_on_edge(Side::Left, size, dvec2(0.0, 2.0), 7.0, 1.0, 10.0);
        assert_eq!(side.tip.y, 17.0);
        });
    }

    #[test]
    fn a_pointer_on_an_edge_too_short_for_it_sits_in_the_middle() {
        crate::on_test_cx(|| {
        let p = pointer_on_edge(Side::Right, dvec2(80.0, 30.0), dvec2(0.0, 2.0), 7.0, 1.0, 10.0);
        assert_eq!(p.base, dvec2(1.0, 15.0));
        assert_eq!(p.tip, dvec2(-6.0, 15.0));
        });
    }

    // -- slide_for_pointer ---------------------------------------------------

    /// A pointer kept 18 in from each end, hung off `anchor` in the base
    /// window: placed, slid, and pointed, the way a popup draws one. Gives
    /// back the slid placement and where along the edge the point landed.
    fn aimed(anchor: Rect, size: DVec2, placement: Placement) -> (Placed, f64) {
        let req = PlaceRequest { anchor, size, placement, ..base() };
        let slid = slide_for_pointer(place_overlay(&req), anchor, req.bounds, 18.0);
        let pointer = pointer_on_edge(slid.side, size, slid.arrow_at - slid.rect.pos, 7.0, 1.0, 11.0);
        let point = slid.rect.pos + pointer.tip;
        (slid, if slid.side.is_vertical() { point.x } else { point.y })
    }

    #[test]
    fn a_slide_brings_a_small_anchors_middle_into_reach_from_either_end() {
        crate::on_test_cx(|| {
        let anchor = r(300.0, 100.0, 20.0, 20.0);
        let (slid, point) = aimed(anchor, dvec2(200.0, 60.0), Placement::BOTTOM_START);
        assert_eq!(point, 310.0);
        assert!(slid.rect.pos.x < anchor.pos.x);
        assert_eq!(slid.arrow_at, dvec2(310.0, 124.0));
        let (slid, point) = aimed(anchor, dvec2(200.0, 60.0), Placement::BOTTOM_END);
        assert_eq!(point, 310.0);
        assert!(slid.rect.pos.x + 200.0 > anchor.pos.x + anchor.size.x);
        let (slid, point) = aimed(anchor, dvec2(160.0, 60.0), Placement::LEFT_START);
        assert_eq!(slid.side, Side::Left);
        assert_eq!(point, 110.0);
        assert_eq!(slid.rect.size, dvec2(160.0, 60.0));
        });
    }

    #[test]
    fn a_slide_centres_an_edge_too_short_for_a_pointer_on_the_anchor() {
        crate::on_test_cx(|| {
        let anchor = r(300.0, 100.0, 20.0, 12.0);
        let (slid, point) = aimed(anchor, dvec2(90.0, 30.0), Placement::RIGHT_START);
        assert_eq!(point, 106.0);
        assert_eq!(slid.rect.pos.y, 91.0);
        });
    }

    #[test]
    fn a_slide_aims_at_a_single_point() {
        crate::on_test_cx(|| {
        let press = r(400.0, 300.0, 0.0, 0.0);
        let (_, point) = aimed(press, dvec2(200.0, 60.0), Placement::BOTTOM_START);
        assert_eq!(point, 400.0);
        });
    }

    #[test]
    fn a_slide_leaves_an_anchor_long_enough_lined_up() {
        crate::on_test_cx(|| {
        // Long enough to hold the pointer from its own start: nothing moves.
        let anchor = r(300.0, 100.0, 36.0, 20.0);
        let req = PlaceRequest { anchor, size: dvec2(200.0, 60.0), ..base() };
        let placed = place_overlay(&req);
        assert_eq!(slide_for_pointer(placed, anchor, req.bounds, 18.0), placed);
        // Longer than the popup: the popup keeps the edge it was lined up
        // with, and the pointer stops over the anchor, short of its middle.
        let anchor = r(300.0, 100.0, 80.0, 74.0);
        let (slid, point) = aimed(anchor, dvec2(90.0, 40.0), Placement::RIGHT_START);
        assert_eq!(slid.rect.pos.y, 100.0);
        assert_eq!(point, 122.0);
        });
    }

    #[test]
    fn a_slide_stops_at_the_bounds() {
        crate::on_test_cx(|| {
        // Against the window's left edge the popup cannot move left, so the
        // pointer stops at the end of the straight part.
        let anchor = r(4.0, 100.0, 10.0, 20.0);
        let (slid, point) = aimed(anchor, dvec2(200.0, 60.0), Placement::BOTTOM_START);
        assert_eq!(slid.rect.pos.x, 0.0);
        assert_eq!(point, 18.0);
        // A popup already past the bounds goes no further, and is not pulled
        // back either: the slide is for the pointer, not a second clamp.
        let anchor = r(-30.0, 100.0, 10.0, 20.0);
        let placed = Placed { rect: r(-30.0, 124.0, 200.0, 60.0), side: Side::Bottom, arrow_at: dvec2(-25.0, 124.0) };
        let slid = slide_for_pointer(placed, anchor, r(0.0, 0.0, 800.0, 600.0), 18.0);
        assert_eq!(slid.rect.pos.x, -30.0);
        // An unbounded axis lets it go.
        let slid = slide_for_pointer(placed, anchor, r(0.0, 0.0, 0.0, 600.0), 18.0);
        assert_eq!(slid.rect.pos.x, -25.0 - 18.0);
        });
    }
}

#[cfg(test)]
mod lock_tests {
    use super::*;
    use crate::{
        makepad_draw::cx_draw::CxDraw,
        pill_nav::{PillNavItem, PillNavLink, PillNavWidgetRefExt},
        view::{View, ViewOptimize},
        widget::*,
    };

    /// A pass and a list to draw a page into, the way a window holds one.
    struct Target {
        pass: DrawPass,
        draw_list: DrawList2d,
        overlay: Overlay,
    }

    impl Target {
        fn new(cx: &mut Cx) -> Self {
            let overlay = cx.with_vm(|vm| Overlay::script_new(vm));
            Target { pass: DrawPass::new(cx), draw_list: DrawList2d::new(cx), overlay }
        }

        fn draw(&mut self, cx: &mut Cx, root: &WidgetRef) {
            let size = dvec2(800.0, 600.0);
            self.pass.set_size(cx, size);
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&self.pass, None);
            self.draw_list.begin_always(&mut cx2d);
            self.overlay.begin(&mut cx2d);
            cx2d.begin_root_turtle(size, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            self.overlay.end(&mut cx2d);
            self.draw_list.end(&mut cx2d);
            cx2d.end_pass(&self.pass);
        }
    }

    fn drawn_cx() -> crate::PooledCx {
        crate::checkout_test_cx()
    }

    /// A page of three buttons, the stand-in for any widget that takes the
    /// pointer with one of its areas.
    fn page(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: 800
                    height: 600
                    flow: Down
                    a := Button{text: "A"}
                    b := Button{text: "B"}
                    c := Button{text: "C"}
                }
            });
            WidgetRef::script_from_value(vm, value)
        })
    }

    /// The page swapped out from under an owner that had no `Drop` to hand
    /// its lock over: the list it drew in is drawn again with the next page,
    /// the lock names nothing drawn, and the next event lets go of it.
    #[test]
    fn a_lock_whose_owner_was_swapped_out_goes_with_the_next_event() {
        crate::on_test_cx(|| {
        let mut cx = drawn_cx();
        let mut target = Target::new(&mut cx);
        let old = page(&mut cx);
        target.draw(&mut cx, &old);
        let held = old.widget(&cx, ids!(b)).area();
        cx.sweep_lock(held);
        drop(old);
        let new = page(&mut cx);
        target.draw(&mut cx, &new);
        assert_eq!(cx.sweep_lock_area(), Some(held), "nothing let go of it when its page went");
        release_orphaned_sweep_locks(&mut cx);
        assert_eq!(cx.sweep_lock_area(), None, "the next event does");
        });
    }

    /// An owner that is still drawn keeps its lock: each draw moves the lock
    /// to the area's fresh handle, so it never reads as left behind.
    #[test]
    fn a_lock_whose_owner_still_draws_is_kept() {
        crate::on_test_cx(|| {
        let mut cx = drawn_cx();
        let mut target = Target::new(&mut cx);
        let root = page(&mut cx);
        target.draw(&mut cx, &root);
        let owner = root.widget(&cx, ids!(b)).area();
        cx.sweep_lock(owner);
        for _ in 0..3 {
            target.draw(&mut cx, &root);
            release_orphaned_sweep_locks(&mut cx);
        }
        let owner = root.widget(&cx, ids!(b)).area();
        assert_eq!(cx.sweep_lock_area(), Some(owner), "held by the area the last draw gave it");
        });
    }

    /// Only the lock that names nothing goes. One left under a living lock
    /// goes too, and the living ones keep their order, so the inner overlay
    /// still has the pointer and the outer gets it back when that one lets go.
    #[test]
    fn only_the_locks_that_name_nothing_go_and_the_rest_keep_their_order() {
        crate::on_test_cx(|| {
        let mut cx = drawn_cx();
        let mut target = Target::new(&mut cx);
        let root = page(&mut cx);
        target.draw(&mut cx, &root);
        // The middle of the stack is the last button drawn, so hiding it
        // moves no living area onto the slot its lock names.
        for id in [ids!(a), ids!(c), ids!(b)] {
            let area = root.widget(&cx, id).area();
            cx.sweep_lock(area);
        }
        root.widget(&cx, ids!(c)).set_visible(&mut cx, false);
        target.draw(&mut cx, &root);
        release_orphaned_sweep_locks(&mut cx);
        let a = root.widget(&cx, ids!(a)).area();
        let b = root.widget(&cx, ids!(b)).area();
        assert_eq!(cx.sweep_lock_area(), Some(b), "the innermost living lock is still on top");
        cx.sweep_unlock(b);
        assert_eq!(cx.sweep_lock_area(), Some(a), "the one under it went, the outer one did not");
        cx.sweep_unlock(a);
        assert_eq!(cx.sweep_lock_area(), None);
        });
    }

    /// A lock taken in a list of the owner's own goes as soon as that list is
    /// dropped with it, without waiting for anything to be drawn again.
    #[test]
    fn a_lock_in_a_dropped_draw_list_goes_before_any_redraw() {
        crate::on_test_cx(|| {
        let mut cx = drawn_cx();
        let mut target = Target::new(&mut cx);
        let root = page(&mut cx);
        if let Some(mut view) = root.borrow_mut::<View>() {
            view.set_optimize(&mut cx, ViewOptimize::DrawList);
        }
        target.draw(&mut cx, &root);
        let held = root.widget(&cx, ids!(a)).area();
        cx.sweep_lock(held);
        release_orphaned_sweep_locks(&mut cx);
        assert_eq!(cx.sweep_lock_area(), Some(held), "a drawn owner keeps it");
        drop(root);
        release_orphaned_sweep_locks(&mut cx);
        assert_eq!(cx.sweep_lock_area(), None, "its list went with it, and the lock with the list");
        });
    }

    /// The case that turned the window away: a pill nav's panel open when the
    /// page holding it is rebuilt. The bar had no `Drop` to hand its lock
    /// over; the next event after the new page is drawn lets go of it.
    #[test]
    fn a_pill_nav_swapped_out_open_gives_the_pointer_back() {
        crate::on_test_cx(|| {
        let mut cx = drawn_cx();
        let mut target = Target::new(&mut cx);
        let old = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{width: 800 height: 600 flow: Down nav := PillNav{}}
            });
            WidgetRef::script_from_value(vm, value)
        });
        let nav = old.widget(&cx, ids!(nav)).as_pill_nav();
        nav.set_items(
            &mut cx,
            vec![
                PillNavItem::place(live_id!(overview), "Overview"),
                PillNavItem::group(live_id!(build), "Build", vec![PillNavLink::new(live_id!(editor), "Editor")]),
            ],
        );
        target.draw(&mut cx, &old);
        nav.open(&mut cx, live_id!(build));
        target.draw(&mut cx, &old);
        assert!(nav.is_open());
        assert!(cx.sweep_lock_area().is_some(), "the open panel holds the pointer");
        drop(nav);
        drop(old);
        let new = page(&mut cx);
        target.draw(&mut cx, &new);
        release_orphaned_sweep_locks(&mut cx);
        assert_eq!(cx.sweep_lock_area(), None, "the next page has the pointer");
        });
    }
}
