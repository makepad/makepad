//! The shelves: the left and the right edge of every display hold the
//! windows put away there, small (the warp law scales them) or in their
//! compact form. This file is the pure part of what the pointer does to a
//! shelf:
//!
//! * the unfold: a click on a tile grows it to its half size in the
//!   shelf's flow and the neighbours are pushed along (compressed when the
//!   shelf is full), a click elsewhere folds it back; nothing follows the
//!   pointer;
//! * the press rules: a press on a shelf item is the WM's until it is known
//!   to be a click (then it unfolds), a drag past the slop pulls the window
//!   out;
//! * the narrow drop zone: a window leaves its shelf a short distance away
//!   from it, wherever on the display the pointer goes.
//!
//! `input.rs` carries the gestures, `desk.rs` draws the poses.
use super::Pose;
use crate::layout::LRect;
use crate::motion::Ease;
use std::f64::consts::PI;

const GAP: f64 = super::GAP;

/// A press on a shelf item that moves this far (points) is a drag.
pub const CLICK_SLOP: f64 = 5.0;
/// A window being dragged leaves its shelf once the pointer is this far
/// (points) from the shelf's inner edge.
pub const LEAVE_DISTANCE: f64 = 32.0;

/// Has a press that began at `start` become a drag at `at`?
pub fn press_is_drag(start: (f64, f64), at: (f64, f64)) -> bool {
    (at.0 - start.0).hypot(at.1 - start.1) >= CLICK_SLOP
}

/// What a press on a tiny tile came to, from the pointer samples between the
/// press and the release: a click (unfold) unless any sample went past the
/// slop, from the press point, which is a drag (pull-out, at that sample).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PressOutcome {
    Click,
    /// Index of the sample that began the drag.
    Drag(usize),
}

pub fn classify_press(start: (f64, f64), samples: &[(f64, f64)]) -> PressOutcome {
    match samples.iter().position(|s| press_is_drag(start, *s)) {
        Some(i) => PressOutcome::Drag(i),
        None => PressOutcome::Click,
    }
}

/// The inner edge of a shelf: the shown x of the innermost item's inner
/// side (the shelf lies between the screen edge and it), none when empty.
pub fn inner_edge(side: i8, shown: impl IntoIterator<Item = LRect>) -> Option<f64> {
    let edges = shown.into_iter().map(|r| if side < 0 { r.x + r.w } else { r.x });
    if side < 0 { edges.reduce(f64::max) } else { edges.reduce(f64::min) }
}

/// Is the pointer at `x` far enough from the shelf (whose inner edge is at
/// `inner`) for a window dragged from it to leave? Only the distance away
/// from the shelf counts, so the window can be taken anywhere on the display.
pub fn leaves_shelf(side: i8, inner: f64, x: f64) -> bool {
    let away = if side < 0 { x - inner } else { inner - x };
    away >= LEAVE_DISTANCE
}

/// The band along the display edge that calls a hidden shelf (points).
pub const EDGE: f64 = 4.0;
/// A dragged window's pointer within this many points of a screen edge is
/// at the edge, and dwelling there this long previews the shelf.
pub const EDGE_PX: f64 = 4.0;
/// How far around a shown shelf the pointer still counts as using it.
pub const KEEP_MARGIN: f64 = 24.0;
/// A hidden shelf is moved this much further than its width: its windows'
/// shadows must be off the display too.
pub const HIDE_SLACK: f64 = 30.0;

/// Does a pointer at `p` keep (or call) the shelf on `side` of the display
/// `screen`, whose items reach `reach` points in from the edge? A hidden
/// shelf answers to the thin band along the edge; a shown one to its
/// footprint with a margin, so leaving takes a deliberate move.
pub fn autohide_over(side: i8, shown: bool, reach: f64, screen: LRect, p: (f64, f64)) -> bool {
    // The area may sit inside the display (outer gaps): a pointer past its
    // edge is at the display's edge, still calling the shelf.
    if p.1 < screen.y || p.1 > screen.y + screen.h {
        return false;
    }
    let from_edge = if side < 0 { p.0 - screen.x } else { screen.x + screen.w - p.0 };
    from_edge <= if shown { reach + KEEP_MARGIN } else { EDGE }
}

/// How far a shelf is moved out of the display when `hidden` (0 shown, 1
/// away) of it is hidden, signed: left shelves go left, right ones right.
pub fn hide_shift(side: i8, hidden: f64, reach: f64) -> f64 {
    hidden.clamp(0.0, 1.0) * (reach + HIDE_SLACK) * side as f64
}

/// Seconds to unfold or fold (the shared ease).
pub const UNFOLD_SECS: f64 = 0.25;
/// The half size is this share of the display's height unless the app was
/// resized before.
pub const HALF_SHARE: f64 = 0.5;
/// The strip along the half window's inner edge (and its bottom corner)
/// where a press resizes it instead of reaching the app, in points.
pub const GRAB: f64 = 6.0;
/// The slim WM strip along the top of a half window whose app draws its own
/// caption: a press on it drags the window (the app's drag query is not asked).
pub const CAPTION_STRIP: f64 = 8.0;

/// Is the point on the half window's top strip (clear of the resize grips)?
pub fn on_caption_strip(r: LRect, p: (f64, f64)) -> bool {
    p.0 >= r.x && p.0 <= r.x + r.w && p.1 >= r.y && p.1 <= r.y + CAPTION_STRIP
}

/// The half size of a window of `aspect` (w / h), tile `tile_h` high, on
/// `screen`: `wanted_h` (the app's remembered height) or half the display's;
/// never smaller than the tile, never taller than `max_h` (the window's full
/// height) or the display, never wider than 60% of the display.
pub fn half_size(aspect: f64, tile_h: f64, wanted_h: Option<f64>, max_h: f64, screen: LRect) -> (f64, f64) {
    let aspect = aspect.max(0.05);
    let cap = max_h.min(screen.h - 2.0 * GAP).min(screen.w * 0.6 / aspect);
    let h = wanted_h.unwrap_or(screen.h * HALF_SHARE).min(cap).max(tile_h.min(cap));
    (h * aspect, h)
}

/// One window unfolded on its shelf: which, on which side of which display,
/// how far it has grown (0 tile .. 1 half) and its half height.
#[derive(Clone, Copy, Debug)]
pub struct Unfold {
    pub client: u64,
    pub side: i8,
    pub screen: LRect,
    pub half_h: f64,
    pub folding: bool,
    /// Room kept clear at the bottom of the display (the dock), in points.
    pub reserve: f64,
    ease: Ease,
}

impl Unfold {
    pub fn new(client: u64, side: i8, screen: LRect, half_h: f64, reserve: f64) -> Unfold {
        let mut ease = Ease::at(0.0, UNFOLD_SECS);
        ease.retarget(1.0);
        Unfold { client, side, screen, half_h, folding: false, reserve, ease }
    }
    /// Grown share now, 0..1.
    pub fn t(&self) -> f64 {
        self.ease.value().clamp(0.0, 1.0)
    }
    /// Open and not on its way back: the window takes its input.
    pub fn open(&self) -> bool {
        !self.folding
    }
    pub fn fold(&mut self) {
        self.folding = true;
        self.ease.retarget(0.0);
    }
    pub fn unfold_again(&mut self) {
        self.folding = false;
        self.ease.retarget(1.0);
    }
    /// Advance; true while moving.
    pub fn step(&mut self, dt: f64) -> bool {
        self.ease.step(dt)
    }
    /// Folded all the way: gone.
    pub fn done(&self) -> bool {
        self.folding && self.ease.settled()
    }
    pub fn busy(&self) -> bool {
        !self.ease.settled()
    }
}

/// Every shelf item's pose with `client` grown to `half` (w, h) by `e`
/// (0 tile .. 1 half): it grows in the shelf's flow from the screen edge,
/// the items of its column are pushed away from it (squeezed toward the
/// display's edges when they would run off), the columns further in move
/// inward by its extra width. Nothing here depends on the pointer.
pub fn unfold_layout(slots: &[(u64, Pose)], client: u64, half: (f64, f64), e: f64, screen: LRect) -> Vec<(u64, Pose)> {
    let Some((_, tile)) = slots.iter().find(|(c, _)| *c == client) else { return slots.to_vec() };
    let side = tile.side;
    let e = e.clamp(0.0, 1.0);
    let (w, h) = (tile.shown.w + (half.0 - tile.shown.w) * e, tile.shown.h + (half.1 - tile.shown.h) * e);
    let x = if side < 0 { tile.shown.x } else { tile.shown.x + tile.shown.w - w };
    let (lo, hi) = (screen.y + GAP, screen.y + screen.h - GAP);
    let cy = tile.shown.y + tile.shown.h * 0.5;
    let y = (cy - h * 0.5).clamp(lo, (hi - h).max(lo));
    let grown = LRect::new(x, y, w, h);
    let k = w / tile.shown.w.max(1e-6);
    let dw = w - tile.shown.w;
    let same_column = |p: &Pose| (p.shown.x + p.shown.w * 0.5 - (tile.shown.x + tile.shown.w * 0.5)).abs() < 0.5 * tile.shown.w.max(p.shown.w);
    let inward = |p: &Pose| if side < 0 { p.shown.x > tile.shown.x + tile.shown.w * 0.5 } else { p.shown.x + p.shown.w < tile.shown.x + tile.shown.w * 0.5 };
    let mut out: Vec<(u64, Pose)> = Vec::with_capacity(slots.len());
    // The column: above the tile, then below, each pushed outward in turn.
    let mut above: Vec<(u64, Pose)> = slots.iter().filter(|(c, p)| *c != client && p.side == side && same_column(p) && p.shown.y + p.shown.h * 0.5 < cy).copied().collect();
    let mut below: Vec<(u64, Pose)> = slots.iter().filter(|(c, p)| *c != client && p.side == side && same_column(p) && p.shown.y + p.shown.h * 0.5 >= cy).copied().collect();
    above.sort_by(|a, b| b.1.shown.y.total_cmp(&a.1.shown.y));
    below.sort_by(|a, b| a.1.shown.y.total_cmp(&b.1.shown.y));
    let mut limit = grown.y - GAP;
    for (_, p) in above.iter_mut() {
        let bottom = p.shown.y + p.shown.h;
        if bottom > limit {
            p.shown.y -= bottom - limit;
        }
        limit = p.shown.y - GAP;
    }
    let mut limit = grown.y + grown.h + GAP;
    for (_, p) in below.iter_mut() {
        if p.shown.y < limit {
            p.shown.y = limit;
        }
        limit = p.shown.y + p.shown.h + GAP;
    }
    // Squeeze what ran past the display toward the tile.
    squeeze(&mut above, grown.y - GAP, lo, true);
    squeeze(&mut below, grown.y + grown.h + GAP, hi, false);
    let mut grown_pose = *tile;
    grown_pose.shown = grown;
    grown_pose.scale = tile.scale * k;
    for (c, p) in slots {
        if *c == client {
            out.push((*c, grown_pose));
        } else if let Some((_, q)) = above.iter().chain(below.iter()).find(|(i, _)| i == c) {
            out.push((*c, *q));
        } else if p.side == side && inward(p) {
            let mut q = *p;
            q.shown.x += dw * -(side as f64);
            out.push((*c, q));
        } else {
            out.push((*c, *p));
        }
    }
    out
}

/// Scale a stack of items about `anchor` (its end next to the unfolded
/// tile) so its far end reaches no further than `limit` (`up`: the stack
/// runs toward smaller y). Sizes shrink with distance, the outer edge stays.
fn squeeze(stack: &mut [(u64, Pose)], anchor: f64, limit: f64, up: bool) {
    let Some(far) = stack.last().map(|(_, p)| if up { p.shown.y } else { p.shown.y + p.shown.h }) else { return };
    let extent = (far - anchor).abs();
    let room = (limit - anchor).abs();
    if extent <= room || extent < 1e-6 || (up && far >= limit) || (!up && far <= limit) {
        return;
    }
    let f = (room / extent).clamp(0.05, 1.0);
    for (_, p) in stack.iter_mut() {
        let (top, bottom) = (anchor + (p.shown.y - anchor) * f, anchor + (p.shown.y + p.shown.h - anchor) * f);
        let (nt, nb) = if top < bottom { (top, bottom) } else { (bottom, top) };
        let w = p.shown.w * f;
        if p.side > 0 {
            p.shown.x += p.shown.w - w;
        }
        p.shown = LRect::new(p.shown.x, nt, w, nb - nt);
        p.scale *= f;
    }
}

/// The corner or edge of the half window's rect a press grabs for resizing,
/// on the shelf `side` (left shelf: the right edge and bottom-right corner;
/// the right shelf mirrors): `Some(true)` the corner, `Some(false)` the edge.
pub fn resize_grab(side: i8, r: LRect, p: (f64, f64)) -> Option<bool> {
    let inner = if side < 0 { r.x + r.w } else { r.x };
    let from_inner = if side < 0 { inner - p.0 } else { p.0 - inner };
    if p.1 < r.y || p.1 > r.y + r.h + GRAB * 0.5 || from_inner < -GRAB * 0.5 || from_inner > GRAB {
        return None;
    }
    let corner = p.1 >= r.y + r.h - GRAB * 2.0;
    Some(corner)
}

/// The half height after a resize drag: the inner edge follows the pointer
/// (`dx` along the shelf's inward direction) or, for the corner, the larger
/// of the two moves; the aspect (w / h) is kept. Clamped by `half_size`.
pub fn resized_h(start_h: f64, aspect: f64, side: i8, dx: f64, dy: f64, corner: bool) -> f64 {
    let dx = dx * -(side as f64);
    let by_w = start_h + dx / aspect.max(0.05);
    if corner { by_w.max(start_h + dy) } else { by_w }
}

/// The auto-hide of every display's two shelves: whether each side hides
/// (one choice for all displays) and one state machine per display and side.
#[derive(Clone, Debug, Default)]
pub struct ShelfHiding {
    enabled: [bool; 2],
    per: Vec<[crate::dock_autohide::DockAutohide; 2]>,
}

impl ShelfHiding {
    pub fn new(enabled: [bool; 2]) -> ShelfHiding {
        ShelfHiding { enabled, per: Vec::new() }
    }
    pub fn enabled(&self, side: usize) -> bool {
        self.enabled[side]
    }
    pub fn any_enabled(&self) -> bool {
        self.enabled.iter().any(|e| *e)
    }
    pub fn set_enabled(&mut self, side: usize, on: bool, now: f64) {
        self.enabled[side] = on;
        for d in &mut self.per {
            d[side].set_enabled(on, now);
        }
    }
    /// Display `i`'s two machines (made on first ask).
    pub fn display(&mut self, i: usize, now: f64) -> &mut [crate::dock_autohide::DockAutohide; 2] {
        while self.per.len() <= i {
            let mut d = [crate::dock_autohide::DockAutohide::default(), crate::dock_autohide::DockAutohide::default()];
            for (side, m) in d.iter_mut().enumerate() {
                m.set_enabled(self.enabled[side], now);
            }
            self.per.push(d);
        }
        &mut self.per[i]
    }
    pub fn step(&mut self, dt: f64) {
        for d in &mut self.per {
            for m in d {
                m.step(dt);
            }
        }
    }
    pub fn fire(&mut self, now: f64) {
        for d in &mut self.per {
            for m in d {
                m.fire(now);
            }
        }
    }
    pub fn sliding(&self) -> bool {
        self.per.iter().any(|d| d.iter().any(|m| m.sliding()))
    }
    pub fn deadline(&self) -> Option<f64> {
        self.per.iter().flat_map(|d| d.iter()).filter_map(|m| m.deadline()).reduce(f64::min)
    }
    /// How much of display `i`'s left and right shelf is hidden, 0..1.
    pub fn hidden(&self, i: usize) -> [f64; 2] {
        self.per.get(i).map_or([0.0; 2], |d| [1.0 - d[0].out_fraction(), 1.0 - d[1].out_fraction()])
    }
}

/// Seconds a shelf item's call-out lasts (the dock's icon of its app was clicked).
pub const PULSE_SECS: f64 = 1.2;

/// The call-out ring's strength `s` (0..1 through its life): two blinks that
/// die away, 0 at both ends.
pub fn pulse_alpha(s: f64) -> f64 {
    let s = s.clamp(0.0, 1.0);
    (1.0 - 0.5 * s) * 0.5 * (1.0 - (4.0 * PI * s).cos())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> LRect {
        LRect::new(0.0, 0.0, 1920.0, 1080.0)
    }

    fn pose(side: i8, x: f64, y: f64, w: f64, h: f64, compact: bool) -> Pose {
        Pose { shown: LRect::new(x, y, w, h), scale: if compact { 1.0 } else { 0.3 }, recede: 0.0, side, compact }
    }


    fn col(n: usize, side: i8) -> Vec<(u64, Pose)> {
        let x = if side < 0 { 10.0 } else { 1750.0 };
        (0..n).map(|i| (i as u64, pose(side, x, 80.0 + i as f64 * 130.0, 160.0, 120.0, false))).collect()
    }

    fn overlap(a: LRect, b: LRect) -> bool {
        a.x < b.x + b.w - 1e-6 && b.x < a.x + a.w - 1e-6 && a.y < b.y + b.h - 1e-6 && b.y < a.y + a.h - 1e-6
    }

    #[test]
    fn unfolding_grows_in_the_flow_pushes_the_column_and_never_overlaps() {
        let slots = col(5, -1);
        let half = (480.0, 360.0);
        for e in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let got = unfold_layout(&slots, 2, half, e, screen());
            for i in 0..got.len() {
                for j in i + 1..got.len() {
                    assert!(!overlap(got[i].1.shown, got[j].1.shown), "e {e}: {:?} {:?}", got[i].1.shown, got[j].1.shown);
                }
                let r = got[i].1.shown;
                assert!(r.y >= screen().y - 1e-6 && r.y + r.h <= screen().y + screen().h + 1e-6, "{r:?}");
            }
            let g = got[2].1.shown;
            assert!((g.w - (160.0 + 320.0 * e)).abs() < 1e-9 && (g.h - (120.0 + 240.0 * e)).abs() < 1e-9);
            assert_eq!(g.x, 10.0, "grows from the screen edge");
        }
        // At rest nothing moved; fully open the neighbours made room.
        let rest = unfold_layout(&slots, 2, half, 0.0, screen());
        assert_eq!(rest.iter().map(|(_, p)| p.shown).collect::<Vec<_>>(), slots.iter().map(|(_, p)| p.shown).collect::<Vec<_>>());
        let open = unfold_layout(&slots, 2, half, 1.0, screen());
        assert!(open[3].1.shown.y > slots[3].1.shown.y && open[1].1.shown.y < slots[1].1.shown.y);
    }

    #[test]
    fn a_full_shelf_squeezes_its_neighbours_instead_of_leaving_the_display() {
        let slots = col(8, -1);
        let open = unfold_layout(&slots, 4, (600.0, 450.0), 1.0, screen());
        for (_, p) in &open {
            assert!(p.shown.y >= screen().y - 1e-6 && p.shown.y + p.shown.h <= screen().y + screen().h + 1e-6, "{:?}", p.shown);
        }
        assert!(open[7].1.shown.h < 120.0, "the far ones are squeezed");
        for i in 0..open.len() {
            for j in i + 1..open.len() {
                assert!(!overlap(open[i].1.shown, open[j].1.shown));
            }
        }
    }

    #[test]
    fn the_right_shelf_mirrors_and_inner_columns_move_inward_by_the_extra_width() {
        let mut slots = col(3, 1);
        slots.push((9, pose(1, 1560.0, 80.0, 160.0, 120.0, false)));
        let open = unfold_layout(&slots, 0, (480.0, 360.0), 1.0, screen());
        let g = open[0].1.shown;
        assert!((g.x + g.w - 1910.0).abs() < 1e-9, "grows from the right edge: {g:?}");
        let inner = open[3].1.shown;
        assert!((inner.x - (1560.0 - 320.0)).abs() < 1e-9, "{inner:?}");
    }

    #[test]
    fn the_half_size_keeps_the_aspect_and_is_clamped() {
        let s = screen();
        let (w, h) = half_size(1.5, 120.0, None, 900.0, s);
        assert!((h - 540.0).abs() < 1e-9 && (w - 810.0).abs() < 1e-9);
        assert_eq!(half_size(1.5, 120.0, Some(10.0), 900.0, s).1, 120.0, "never below the tile");
        assert_eq!(half_size(1.5, 120.0, Some(5000.0), 700.0, s).1, 700.0, "never above the full size");
        let wide = half_size(4.0, 120.0, Some(900.0), 1000.0, s);
        assert!(wide.0 <= s.w * 0.6 + 1e-9, "{wide:?}");
    }

    #[test]
    fn the_unfold_state_opens_folds_and_ends() {
        let mut u = Unfold::new(3, -1, screen(), 400.0, 0.0);
        assert!(u.open() && u.t() < 0.01 && u.busy());
        while u.step(1.0 / 60.0) {}
        assert!((u.t() - 1.0).abs() < 1e-9 && u.open() && !u.done());
        u.fold();
        assert!(!u.open() && !u.done());
        let mut last = 1.0;
        while u.step(1.0 / 60.0) {
            assert!(u.t() <= last + 1e-9);
            last = u.t();
        }
        assert!(u.done() && u.t() < 1e-9);
        // A change of mind mid-fold bends back.
        let mut v = Unfold::new(3, -1, screen(), 400.0, 0.0);
        while v.step(1.0 / 60.0) {}
        v.fold();
        for _ in 0..5 {
            v.step(1.0 / 60.0);
        }
        v.unfold_again();
        while v.step(1.0 / 60.0) {}
        assert!((v.t() - 1.0).abs() < 1e-9 && !v.done());
    }

    #[test]
    fn only_the_inner_edge_and_bottom_corner_of_the_half_window_resize() {
        let r = LRect::new(10.0, 200.0, 480.0, 360.0);
        // Left shelf: the right edge.
        assert_eq!(resize_grab(-1, r, (488.0, 300.0)), Some(false));
        assert_eq!(resize_grab(-1, r, (488.0, 556.0)), Some(true));
        assert_eq!(resize_grab(-1, r, (300.0, 300.0)), None, "the body is the app's");
        assert_eq!(resize_grab(-1, r, (12.0, 300.0)), None, "the outer edge is not a grip");
        assert_eq!(resize_grab(-1, r, (300.0, 556.0)), None, "nor the bottom edge");
        // Right shelf mirrors.
        let r = LRect::new(1430.0, 200.0, 480.0, 360.0);
        assert_eq!(resize_grab(1, r, (1432.0, 300.0)), Some(false));
        assert_eq!(resize_grab(1, r, (1432.0, 556.0)), Some(true));
        assert_eq!(resize_grab(1, r, (1700.0, 300.0)), None);
    }

    #[test]
    fn the_caption_strip_is_the_top_edge_of_the_half_window_only() {
        let r = LRect::new(10.0, 200.0, 480.0, 360.0);
        assert!(on_caption_strip(r, (200.0, 203.0)));
        assert!(!on_caption_strip(r, (200.0, 209.0)));
        assert!(!on_caption_strip(r, (200.0, 199.0)));
        assert!(!on_caption_strip(r, (5.0, 203.0)));
    }

    #[test]
    fn a_resize_drag_follows_the_pointer_inward_and_keeps_the_aspect() {
        // Left shelf, dragging the right edge 150 right on a 4:3 window.
        let h = resized_h(360.0, 4.0 / 3.0, -1, 150.0, 0.0, false);
        assert!((h - (360.0 + 150.0 * 0.75)).abs() < 1e-9, "{h}");
        // Right shelf: dragging left grows it.
        assert!(resized_h(360.0, 4.0 / 3.0, 1, -150.0, 0.0, false) > 360.0);
        // The corner takes the larger of the two.
        assert_eq!(resized_h(360.0, 1.0, -1, 10.0, 80.0, true), 440.0);
    }

    #[test]
    fn the_call_out_blinks_twice_and_dies_away() {
        assert_eq!(pulse_alpha(0.0), 0.0);
        assert_eq!(pulse_alpha(1.0), 0.0);
        let (first, second) = (pulse_alpha(0.25), pulse_alpha(0.75));
        assert!(first > 0.8 && second < first && second > 0.4, "{first} {second}");
        assert!((0.0..=1.0).contains(&pulse_alpha(0.3)));
        assert!(pulse_alpha(0.5) < 1e-9);
    }

    #[test]
    fn press_sequences_on_a_tile_are_clicks_or_drags_by_the_slop_alone() {
        let p = (100.0, 100.0);
        // No movement, and a wobble inside the slop: a click.
        assert_eq!(classify_press(p, &[]), PressOutcome::Click);
        assert_eq!(classify_press(p, &[(101.0, 100.0), (99.0, 102.0), (103.0, 103.0)]), PressOutcome::Click);
        // A slow drag in any direction starts the pull-out at the first sample past the slop.
        for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0), (0.8, 0.8)] {
            let samples: Vec<(f64, f64)> = (1..30).map(|i| (p.0 + dx * i as f64, p.1 + dy * i as f64)).collect();
            assert_eq!(classify_press(p, &samples), PressOutcome::Drag(4), "{dx},{dy}");
        }
        // A flick: one big jump is a drag at once, never a click.
        assert_eq!(classify_press(p, &[(300.0, 100.0)]), PressOutcome::Drag(0));
        // A wobble first, then a move: still a drag, at the move.
        assert_eq!(classify_press(p, &[(102.0, 100.0), (101.0, 101.0), (160.0, 100.0)]), PressOutcome::Drag(2));
        // A drag that returns to the start before the release is still a drag.
        assert_eq!(classify_press(p, &[(130.0, 100.0), (100.0, 100.0)]), PressOutcome::Drag(0));
    }

    #[test]
    fn a_press_is_a_click_until_it_moves_the_slop() {
        assert!(!press_is_drag((100.0, 100.0), (100.0, 100.0)));
        assert!(!press_is_drag((100.0, 100.0), (103.0, 103.0)));
        assert!(press_is_drag((100.0, 100.0), (105.0, 100.0)));
        assert!(press_is_drag((100.0, 100.0), (100.0, 94.0)));
    }

    #[test]
    fn the_drop_zone_is_a_few_points_wide_and_anywhere_on_the_display_leaves() {
        let left_items = [LRect::new(10.0, 100.0, 160.0, 120.0), LRect::new(10.0, 300.0, 120.0, 90.0)];
        let right_items = [LRect::new(1750.0, 100.0, 160.0, 120.0), LRect::new(1800.0, 300.0, 110.0, 90.0)];
        assert_eq!(inner_edge(-1, left_items), Some(170.0));
        assert_eq!(inner_edge(1, right_items), Some(1750.0));
        assert_eq!(inner_edge(-1, std::iter::empty()), None);
        // Left shelf: 31 points away stays, 32 leaves; across the whole display too.
        assert!(!leaves_shelf(-1, 170.0, 170.0 + 31.0));
        assert!(leaves_shelf(-1, 170.0, 170.0 + 32.0));
        assert!(leaves_shelf(-1, 170.0, 1900.0));
        assert!(!leaves_shelf(-1, 170.0, 20.0));
        // Right shelf mirrors.
        assert!(!leaves_shelf(1, 1750.0, 1750.0 - 31.0));
        assert!(leaves_shelf(1, 1750.0, 1750.0 - 32.0));
        assert!(leaves_shelf(1, 1750.0, 10.0));
    }

    #[test]
    fn a_hidden_shelf_answers_to_the_edge_and_a_shown_one_to_its_footprint() {
        let s = screen();
        // Hidden left: only the edge band, anywhere along the edge.
        assert!(autohide_over(-1, false, 170.0, s, (0.0, 500.0)));
        assert!(autohide_over(-1, false, 170.0, s, (EDGE, 1.0)));
        assert!(!autohide_over(-1, false, 170.0, s, (EDGE + 1.0, 500.0)));
        // Shown left: the footprint and a margin.
        assert!(autohide_over(-1, true, 170.0, s, (170.0 + KEEP_MARGIN, 500.0)));
        assert!(!autohide_over(-1, true, 170.0, s, (170.0 + KEEP_MARGIN + 1.0, 500.0)));
        // Right mirrors; off the display is never over.
        assert!(autohide_over(1, false, 170.0, s, (1919.0, 10.0)));
        assert!(!autohide_over(1, false, 170.0, s, (1900.0, 10.0)));
        assert!(autohide_over(1, true, 170.0, s, (1920.0 - 190.0, 10.0)));
        assert!(autohide_over(-1, false, 170.0, s, (-5.0, 10.0)), "past the area's edge is at the edge");
        assert!(!autohide_over(-1, true, 170.0, s, (10.0, 2000.0)));
    }

    #[test]
    fn each_display_hides_its_shelves_on_its_own() {
        let mut h = ShelfHiding::new([true, false]);
        h.display(0, 0.0);
        h.display(1, 0.0);
        let mut t = 0.0;
        while t < 1.5 {
            t += 0.01;
            h.fire(t);
            h.step(0.01);
        }
        assert_eq!(h.hidden(0), [1.0, 0.0]);
        assert_eq!(h.hidden(1), [1.0, 0.0]);
        // The pointer touches display 1's left edge only.
        h.display(1, t)[0].update(t, true, false);
        while t < 2.2 {
            t += 0.01;
            h.fire(t);
            h.step(0.01);
        }
        assert_eq!(h.hidden(1), [0.0, 0.0]);
        assert_eq!(h.hidden(0), [1.0, 0.0], "the other display's shelf stays away");
        // A display never asked about is shown.
        assert_eq!(h.hidden(5), [0.0, 0.0]);
        // Turning the choice on later reaches existing displays.
        h.set_enabled(1, true, t);
        assert!(h.display(0, t)[1].enabled && h.display(7, t)[1].enabled);
    }

    #[test]
    fn a_hidden_shelf_moves_wholly_off_its_display_and_shown_does_not_move() {
        assert_eq!(hide_shift(-1, 0.0, 170.0), 0.0);
        assert_eq!(hide_shift(1, 0.0, 170.0), 0.0);
        // Fully hidden: the inner edge of a shelf `reach` wide plus the slack is past the edge.
        assert_eq!(hide_shift(-1, 1.0, 170.0), -(170.0 + HIDE_SLACK));
        assert_eq!(hide_shift(1, 1.0, 170.0), 170.0 + HIDE_SLACK);
        // Monotone between.
        assert!(hide_shift(-1, 0.5, 170.0) > hide_shift(-1, 1.0, 170.0));
    }

}
