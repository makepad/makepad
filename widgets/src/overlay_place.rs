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
        assert_eq!(Side::Top.opposite(), Side::Bottom);
        assert_eq!(Side::Bottom.opposite(), Side::Top);
        assert_eq!(Side::Left.opposite(), Side::Right);
        assert_eq!(Side::Right.opposite(), Side::Left);
        assert!(Side::Top.is_vertical());
        assert!(Side::Bottom.is_vertical());
        assert!(!Side::Left.is_vertical());
        assert!(!Side::Right.is_vertical());
    }

    #[test]
    fn fits_below_and_stays_there() {
        let p = place_overlay(&base());
        assert_eq!(p.rect, r(200.0, 224.0, 60.0, 40.0));
        assert_eq!(p.side, Side::Bottom);
        assert_eq!(p.arrow_at, dvec2(250.0, 224.0));
    }

    #[test]
    fn fits_exactly_and_still_stays() {
        // Room below is exactly the popup's height: 600 - (556 + 4) = 40.
        let p = place_overlay(&PlaceRequest {
            anchor: r(200.0, 536.0, 100.0, 20.0),
            ..base()
        });
        assert_eq!(p.side, Side::Bottom);
        assert_eq!(p.rect, r(200.0, 560.0, 60.0, 40.0));
    }

    #[test]
    fn each_of_the_twelve_placements_lands_where_it_says() {
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
    }

    // -- flipping ----------------------------------------------------------

    #[test]
    fn flips_up_when_below_is_short_and_above_fits() {
        // Room below: 600 - (590 + 4) = 6; room above: 566.
        let p = place_overlay(&PlaceRequest {
            anchor: r(200.0, 570.0, 100.0, 20.0),
            ..base()
        });
        assert_eq!(p.side, Side::Top);
        assert_eq!(p.rect, r(200.0, 526.0, 60.0, 40.0));
        assert_eq!(p.arrow_at, dvec2(250.0, 566.0));
    }

    #[test]
    fn flips_left_when_right_is_short_and_left_fits() {
        // Room right: 800 - (780 + 4) = 16; room left: 696.
        let p = place_overlay(&PlaceRequest {
            anchor: r(700.0, 200.0, 80.0, 20.0),
            placement: Placement::RIGHT_START,
            ..base()
        });
        assert_eq!(p.side, Side::Left);
        assert_eq!(p.rect, r(636.0, 200.0, 60.0, 40.0));
        assert_eq!(p.arrow_at, dvec2(696.0, 210.0));
    }

    #[test]
    fn neither_fits_takes_the_roomier_side_and_shrinks_to_it() {
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
    }

    #[test]
    fn a_tie_between_two_short_sides_keeps_the_requested_side() {
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
    }

    #[test]
    fn the_requested_side_wins_when_it_is_roomier_even_though_nothing_fits() {
        // Room below 376, room above 196, popup 1000 tall: no flip, 376 tall.
        let p = place_overlay(&PlaceRequest {
            size: dvec2(60.0, 1000.0),
            ..base()
        });
        assert_eq!(p.side, Side::Bottom);
        assert_eq!(p.rect, r(200.0, 224.0, 60.0, 376.0));
        assert_eq!(p.arrow_at, dvec2(250.0, 224.0));
    }

    #[test]
    fn taller_than_the_window_flips_when_above_is_roomier() {
        // Anchor at y=500: room below 76, room above 496.
        let p = place_overlay(&PlaceRequest {
            anchor: r(200.0, 500.0, 100.0, 20.0),
            size: dvec2(60.0, 1000.0),
            ..base()
        });
        assert_eq!(p.side, Side::Top);
        assert_eq!(p.rect, r(200.0, 0.0, 60.0, 496.0));
        assert_eq!(p.arrow_at, dvec2(250.0, 496.0));
    }

    #[test]
    fn no_room_on_either_side_gives_a_zero_height_not_a_negative_one() {
        // Anchor fills the whole window: both rooms are negative.
        let p = place_overlay(&PlaceRequest {
            anchor: r(0.0, 0.0, 800.0, 600.0),
            ..base()
        });
        assert_eq!(p.rect.size.y, 0.0);
        assert!(p.rect.size.y >= 0.0);
    }

    // -- shifting ----------------------------------------------------------

    #[test]
    fn shifts_inboard_on_the_right_edge() {
        let p = place_overlay(&PlaceRequest {
            anchor: r(770.0, 200.0, 100.0, 20.0),
            ..base()
        });
        assert_eq!(p.rect, r(740.0, 224.0, 60.0, 40.0));
        assert_eq!(p.side, Side::Bottom);
    }

    #[test]
    fn shifts_inboard_on_the_left_edge() {
        let p = place_overlay(&PlaceRequest {
            anchor: r(-30.0, 200.0, 100.0, 20.0),
            ..base()
        });
        assert_eq!(p.rect, r(0.0, 224.0, 60.0, 40.0));
    }

    #[test]
    fn the_low_edge_wins_when_wider_than_the_bounds() {
        let p = place_overlay(&PlaceRequest {
            bounds: r(10.0, 0.0, 50.0, 600.0),
            ..base()
        });
        assert_eq!(p.rect, r(10.0, 224.0, 50.0, 40.0));
    }

    #[test]
    fn end_alignment_shifts_inboard_too() {
        // End puts the popup's right edge on the anchor's, at 870: past 800.
        let p = place_overlay(&PlaceRequest {
            anchor: r(770.0, 200.0, 100.0, 20.0),
            placement: Placement::BOTTOM_END,
            ..base()
        });
        assert_eq!(p.rect, r(740.0, 224.0, 60.0, 40.0));
    }

    #[test]
    fn horizontal_sides_shift_along_y() {
        let p = place_overlay(&PlaceRequest {
            anchor: r(200.0, 590.0, 100.0, 20.0),
            placement: Placement::RIGHT_START,
            ..base()
        });
        assert_eq!(p.side, Side::Right);
        assert_eq!(p.rect, r(304.0, 560.0, 60.0, 40.0));
    }

    #[test]
    fn arrow_at_follows_the_anchor_centre_and_clamps_to_the_popup_edge() {
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
    }

    // -- bounds ------------------------------------------------------------

    #[test]
    fn an_unbounded_request_never_flips_shifts_or_shrinks() {
        let p = place_overlay(&PlaceRequest {
            anchor: r(770.0, 580.0, 100.0, 20.0),
            size: dvec2(60.0, 1000.0),
            bounds: Rect::default(),
            ..base()
        });
        assert_eq!(p.side, Side::Bottom);
        assert_eq!(p.rect, r(770.0, 604.0, 60.0, 1000.0));
    }

    #[test]
    fn a_negative_bounds_extent_is_unbounded_too() {
        let p = place_overlay(&PlaceRequest {
            anchor: r(770.0, 580.0, 100.0, 20.0),
            bounds: r(6.0, 6.0, -12.0, -12.0),
            ..base()
        });
        assert_eq!(p.rect, r(770.0, 604.0, 60.0, 40.0));
    }

    #[test]
    fn each_axis_is_bounded_on_its_own() {
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
    }

    #[test]
    fn bounds_need_not_start_at_the_origin() {
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
    }

    // -- match_anchor_width ------------------------------------------------

    #[test]
    fn match_anchor_width_grows_to_the_anchor_and_never_shrinks() {
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
    }

    #[test]
    fn a_matched_width_still_shrinks_to_the_bounds() {
        let p = place_overlay(&PlaceRequest {
            anchor: r(0.0, 200.0, 1000.0, 20.0),
            match_anchor_width: true,
            ..base()
        });
        assert_eq!(p.rect, r(0.0, 224.0, 800.0, 40.0));
    }

    // -- span_inboard --------------------------------------------------------

    #[test]
    fn span_inboard_leaves_a_span_that_fits_alone() {
        assert_eq!(span_inboard(100.0, 50.0, 8.0, 784.0), 100.0);
        assert_eq!(span_inboard(8.0, 50.0, 8.0, 784.0), 8.0);
        assert_eq!(span_inboard(742.0, 50.0, 8.0, 784.0), 742.0);
    }

    #[test]
    fn span_inboard_pulls_back_from_the_high_edge() {
        assert_eq!(span_inboard(780.0, 50.0, 8.0, 784.0), 742.0);
    }

    #[test]
    fn span_inboard_pulls_forward_from_the_low_edge() {
        assert_eq!(span_inboard(-20.0, 50.0, 8.0, 784.0), 8.0);
    }

    #[test]
    fn span_inboard_lets_the_low_edge_win_when_too_long() {
        assert_eq!(span_inboard(300.0, 900.0, 8.0, 784.0), 8.0);
        assert_eq!(span_inboard(-300.0, 900.0, 8.0, 784.0), 8.0);
    }

    #[test]
    fn span_inboard_is_a_no_op_without_room() {
        assert_eq!(span_inboard(300.0, 50.0, 8.0, 0.0), 300.0);
        assert_eq!(span_inboard(-300.0, 50.0, 8.0, -12.0), -300.0);
    }

    #[test]
    fn span_inboard_is_the_one_line_the_sites_had() {
        // ComboBox and DropDown2 wrote `x.clamp(m, (pass - m - w).max(m))`.
        let cases: [(f64, f64, f64); 3] = [(360.0, 200.0, 400.0), (-5.0, 30.0, 400.0), (50.0, 500.0, 400.0)];
        for (x, w, pass) in cases {
            let m = 8.0_f64;
            let old = x.clamp(m, (pass - m - w).max(m));
            assert_eq!(span_inboard(x, w, m, pass - m * 2.0), old, "x={x} w={w}");
        }
    }

    // -- pointer_on_edge -----------------------------------------------------

    #[test]
    fn a_pointer_stands_out_of_the_edge_that_faces_the_anchor() {
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
    }

    #[test]
    fn a_pointer_keeps_its_whole_base_off_the_corners() {
        let size = dvec2(100.0, 60.0);
        // Aimed past either end: the base's near end stops where the corner does.
        let low = pointer_on_edge(Side::Bottom, size, dvec2(-40.0, 0.0), 7.0, 1.0, 10.0);
        assert_eq!(low.tip.x, 17.0);
        let high = pointer_on_edge(Side::Bottom, size, dvec2(400.0, 0.0), 7.0, 1.0, 10.0);
        assert_eq!(high.tip.x, 83.0);
        let side = pointer_on_edge(Side::Left, size, dvec2(0.0, 2.0), 7.0, 1.0, 10.0);
        assert_eq!(side.tip.y, 17.0);
    }

    #[test]
    fn a_pointer_on_an_edge_too_short_for_it_sits_in_the_middle() {
        let p = pointer_on_edge(Side::Right, dvec2(80.0, 30.0), dvec2(0.0, 2.0), 7.0, 1.0, 10.0);
        assert_eq!(p.base, dvec2(1.0, 15.0));
        assert_eq!(p.tip, dvec2(-6.0, 15.0));
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
    }

    #[test]
    fn a_slide_centres_an_edge_too_short_for_a_pointer_on_the_anchor() {
        let anchor = r(300.0, 100.0, 20.0, 12.0);
        let (slid, point) = aimed(anchor, dvec2(90.0, 30.0), Placement::RIGHT_START);
        assert_eq!(point, 106.0);
        assert_eq!(slid.rect.pos.y, 91.0);
    }

    #[test]
    fn a_slide_aims_at_a_single_point() {
        let press = r(400.0, 300.0, 0.0, 0.0);
        let (_, point) = aimed(press, dvec2(200.0, 60.0), Placement::BOTTOM_START);
        assert_eq!(point, 400.0);
    }

    #[test]
    fn a_slide_leaves_an_anchor_long_enough_lined_up() {
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
    }

    #[test]
    fn a_slide_stops_at_the_bounds() {
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
    }
}
