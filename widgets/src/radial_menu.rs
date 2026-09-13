//! RadialMenu — a ring of choices around a point, where a choice can hold
//! choices of its own.
//!
//! A choice with children opens an outer ring on the same centre, its arc
//! centred on the parent wedge's own direction, so the hand keeps travelling
//! the way it was already going: one continuous flick outward picks a choice
//! two or three rings deep. Only the direction within the open rings decides
//! the pick; how far past a ring's inner edge the pointer went never does.
//!
//! It floats over everything on an overlay of its own. That lets the rings
//! reach past the widget that opened them, and it is what makes the frosted
//! look possible, because glass can only sample the window from an overlay.
//!
//! The rings grow open on the theme's motion tokens: `enter_ease` is one of
//! the theme's easings and both enter times are theme durations, so the menu
//! moves the way the rest of the app does. Only the drawing follows the
//! curve. A spring carries a ring past its edge and back, and the pick, the
//! digits and the outside test go on reading the edge the ring comes to rest
//! at, so what a pointer reaches never depends on the frame it lands in.
//!
//! `PieMenu` stays what it is: the pinned, in-field ring of flat labels that
//! reports an index. This is the summoned menu with depth. It shares the pie
//! menu's angle measure and its number keys, not its drawing.
//!
//! What it deliberately does NOT do. No more than three rings: a fourth
//! would need a disc wider than most windows are tall, and by then its arcs
//! have been split three times. No arrow keys, for the pie menu's reason: in
//! a ring an arrow would have to be a compass direction and a list step at
//! once. No memory of the last pick, and no pinned mode, which is the pie
//! menu's job. Nothing animates away: a pick, a cancel or a move inward takes
//! its rings down at once, because a ring still shrinking under the pointer
//! reads as a ring that is still open.

use crate::{
    animator::Ease,
    badge::measure,
    event::TouchState,
    gauss_view::{arm_gauss_capture, bind_gauss_snapshot, request_window_gauss, GaussBlurSnapshot},
    makepad_derive_widget::*,
    makepad_draw::*,
    modal::area_after_redraws,
    overlay_place::{claim_escape, orphan_sweep_locks},
    pie_menu::{fit_centre, key_number},
    widget::*,
};
use std::f64::consts::{PI, TAU};

/// Ring 0 and two outer rings. With the default radii a full-depth menu is
/// a 472-point disc, which fits a 600-point window with a margin; a fourth
/// ring would not, and its arcs would be too thin to aim at anyway.
const MAX_DEPTH: usize = 3;

/// The least ring 0 can reach past its hub and still be aimed at: the pie
/// menu's floor, so the two menus answer a hub wider than the radius alike.
const RING_MIN: f64 = 8.0;

/// Room around each wedge's own box for the feather along its edges.
const WEDGE_PAD: f64 = 2.0;

/// How far the rings keep in from the window's usable edge, the popover's
/// margin, so a menu and a popover opened at the same corner line up.
const EDGE: f64 = 6.0;

/// How far below the top of its line box a glyph's ink starts, as a share of
/// the font size. `draw_abs` takes the line box, so centring the box alone
/// leaves every label riding high.
const INK_DROP: f64 = 0.30;

/// Where a wedge's number would sit between its inner and outer edge with
/// nothing in its way: near the inner edge, on the wedge's middle line. A
/// word laid out across a wedge that points sideways reaches in past that
/// spot, so the number starts here and moves off the word (see
/// `number_spot`).
const NUMBER_AT: f64 = 0.22;

/// The clear space a number keeps from every word and glyph, and from its
/// wedge's edges and seams.
const NUMBER_CLEAR: f64 = 2.0;

/// How finely a curve is walked for the furthest it swings. On the theme's
/// spring the peak found is within a millionth of the true one, far under a
/// point on the widest ring the widget allows.
const PEAK_SAMPLES: usize = 1024;

/// One choice, with the choices it opens.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RadialNode {
    /// `LiveId::from_str(&key)`.
    pub id: LiveId,
    /// The full path, segments joined by '/': "share/mail".
    pub key: String,
    pub label: String,
    /// An optional glyph in the icon face; empty for none.
    pub icon: String,
    pub enabled: bool,
    pub children: Vec<RadialNode>,
}

impl RadialNode {
    /// A choice named by ONE key segment. `children` gives it its full path
    /// once it is placed under a parent.
    pub fn new(key: &str, label: &str) -> Self {
        Self {
            id: LiveId::from_str(key),
            key: key.to_string(),
            label: label.to_string(),
            icon: String::new(),
            enabled: true,
            children: Vec::new(),
        }
    }

    pub fn icon(mut self, glyph: &str) -> Self {
        self.icon = glyph.to_string();
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Place `nodes` under this choice. Their keys, and their children's,
    /// are rewritten to start with this key, because a pick reports the full
    /// path and "mail" alone could be under two parents.
    pub fn children(mut self, nodes: Vec<RadialNode>) -> Self {
        let parent = self.key.clone();
        self.children = nodes
            .into_iter()
            .map(|mut node| {
                node.prefix(&parent);
                node
            })
            .collect();
        self
    }

    fn prefix(&mut self, parent: &str) {
        self.key = format!("{parent}/{}", self.key);
        self.id = LiveId::from_str(&self.key);
        for child in &mut self.children {
            child.prefix(parent);
        }
    }
}

/// How many levels a tree has: 0 for no choices, 1 for a single ring.
fn levels(nodes: &[RadialNode]) -> usize {
    nodes
        .iter()
        .map(|node| 1 + levels(&node.children))
        .max()
        .unwrap_or(0)
}

/// Drop everything deeper than `MAX_DEPTH`, answering the keys it dropped.
fn prune(nodes: &mut [RadialNode], depth: usize, dropped: &mut Vec<String>) {
    for node in nodes {
        if depth + 1 >= MAX_DEPTH {
            dropped.extend(node.children.drain(..).map(|child| child.key));
        } else {
            prune(&mut node.children, depth + 1, dropped);
        }
    }
}

/// One item as the DSL declares it. The list is flat, keys carrying the
/// nesting, so no derive has to handle a type that contains itself.
#[derive(Script, ScriptHook, Default)]
pub struct RadialItem {
    #[source]
    source: ScriptObjectRef,
    /// The full path, segments joined by '/'.
    #[live]
    pub key: String,
    #[live]
    pub label: String,
    #[live]
    pub icon: String,
    #[live(true)]
    pub enabled: bool,
}

/// Turn a flat `(key, label, icon, enabled)` list into a tree, answering the
/// keys that were dropped, in declaration order.
///
/// A key is dropped when it has an empty segment, is deeper than three
/// rings, names a parent that was not already kept, or was already kept (the
/// first one wins). Siblings keep their declaration order, which is the
/// clockwise order they are drawn in. A parent has to come first because a
/// reader scans the list top to bottom; a child written above its parent
/// reads as a mistake, and quietly accepting it would hide one.
pub fn build_tree(flat: &[(&str, &str, &str, bool)]) -> (Vec<RadialNode>, Vec<String>) {
    let mut tree: Vec<RadialNode> = Vec::new();
    let mut kept: Vec<&str> = Vec::new();
    let mut dropped = Vec::new();
    'items: for &(key, label, icon, enabled) in flat {
        let segments: Vec<&str> = key.split('/').collect();
        let parent = key.rfind('/').map(|at| &key[..at]);
        let keep = !segments.iter().any(|segment| segment.is_empty())
            && segments.len() <= MAX_DEPTH
            && parent.map_or(true, |parent| kept.contains(&parent))
            && !kept.contains(&key);
        if !keep {
            dropped.push(key.to_string());
            continue;
        }
        let mut siblings = &mut tree;
        for depth in 1..segments.len() {
            let parent_key = segments[..depth].join("/");
            let here = siblings;
            let Some(at) = here.iter().position(|node| node.key == parent_key) else {
                dropped.push(key.to_string());
                continue 'items;
            };
            siblings = &mut here[at].children;
        }
        siblings.push(RadialNode {
            id: LiveId::from_str(key),
            key: key.to_string(),
            label: label.to_string(),
            icon: icon.to_string(),
            enabled,
            children: Vec::new(),
        });
        kept.push(key);
    }
    (tree, dropped)
}

/// One ring's worth of wedges: `count` of them sharing `span` radians from
/// `start`, between `inner` and `outer` points from the centre.
///
/// Angles are the pie menu's measure — radians CLOCKWISE FROM STRAIGHT UP —
/// so a full ring here and a `PieRing` agree on every wedge. An outer ring
/// is the same thing with less than the whole circle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArcRing {
    pub count: usize,
    pub start: f64,
    pub span: f64,
    pub inner: f64,
    pub outer: f64,
}

impl ArcRing {
    /// The whole circle, wedge 0 centred on twelve o'clock as the pie menu
    /// draws it.
    pub fn full(count: usize, inner: f64, outer: f64) -> Self {
        let start = if count == 0 { 0.0 } else { -PI / count as f64 };
        Self { count, start, span: TAU, inner, outer }
    }

    /// The outer ring a parent opens: `count` children between `inner` and
    /// `outer`, on an arc whose middle is exactly the parent's own direction.
    ///
    /// The arc starts as wide as the parent wedge and widens only until
    /// every child gets `min_item_arc` points at the ring's middle radius. It
    /// widens about its middle, never from one end, because the middle is
    /// the direction the hand is already moving in; a ring that grew from
    /// one side would put the first child where the parent was and send the
    /// flick sideways.
    pub fn child(
        parent: &ArcRing,
        index: usize,
        count: usize,
        inner: f64,
        outer: f64,
        min_item_arc: f64,
    ) -> Self {
        let mid = ((inner + outer) * 0.5).max(1.0);
        let min_span = count as f64 * min_item_arc.max(0.0) / mid;
        let span = parent.step().max(min_span).min(TAU);
        let start = (parent.centre(index) - span * 0.5).rem_euclid(TAU);
        Self { count, start, span, inner, outer }
    }

    /// The angle one wedge takes; the whole span when there are none.
    pub fn step(&self) -> f64 {
        if self.count == 0 {
            self.span
        } else {
            self.span / self.count as f64
        }
    }

    /// The middle of wedge `i`.
    pub fn centre(&self, i: usize) -> f64 {
        (self.start + (i as f64 + 0.5) * self.step()).rem_euclid(TAU)
    }

    /// Where wedge `i` begins and ends, both in `0..TAU`. For a wedge that
    /// straddles twelve o'clock the first is the LARGER, and a caller that
    /// assumes otherwise draws the rest of the circle instead of the wedge.
    pub fn span_of(&self, i: usize) -> (f64, f64) {
        let step = self.step();
        (
            (self.start + i as f64 * step).rem_euclid(TAU),
            (self.start + (i + 1) as f64 * step).rem_euclid(TAU),
        )
    }

    /// The wedge a direction falls in, or nothing when it is outside the
    /// arc. A full ring answers every direction.
    pub fn angle_index(&self, a: f64) -> Option<usize> {
        if self.count == 0 {
            return None;
        }
        let t = (a - self.start).rem_euclid(TAU);
        if self.span < TAU && t >= self.span {
            return None;
        }
        Some(((t / self.step()).floor() as usize).min(self.count - 1))
    }

    /// How far right of and BELOW the centre the middle of wedge `i` sits,
    /// `radius` out, as `PieRing::offset` measures it.
    pub fn offset(&self, i: usize, radius: f64) -> (f64, f64) {
        let c = self.centre(i);
        (c.sin() * radius, -c.cos() * radius)
    }
}

/// The radii every ring sits at, from the widget's four numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RingBands {
    hub: f64,
    r0: f64,
    width: f64,
    gap: f64,
}

impl RingBands {
    fn new(hub_radius: f64, radius: f64, ring_width: f64, ring_gap: f64) -> Self {
        let hub = hub_radius.max(0.0);
        Self {
            hub,
            r0: radius.max(hub + RING_MIN),
            width: ring_width.max(1.0),
            gap: ring_gap.max(0.0),
        }
    }

    /// Inner and outer edge of the ring at `depth`.
    fn band(&self, depth: usize) -> (f64, f64) {
        let mut band = (self.hub, self.r0);
        for _ in 0..depth {
            let inner = band.1 + self.gap;
            band = (inner, inner + self.width);
        }
        band
    }

    fn reach(&self, depth: usize) -> f64 {
        self.band(depth).1
    }
}

/// The choices on the ring at `depth`, following `open_path` down.
fn ring_nodes<'a>(tree: &'a [RadialNode], open_path: &[usize], depth: usize) -> Option<&'a [RadialNode]> {
    let mut nodes = tree;
    for k in 0..depth {
        nodes = &nodes.get(*open_path.get(k)?)?.children;
    }
    Some(nodes)
}

/// Every ring that is open: ring 0, then one per parent in `open_path` for
/// as long as that path still names a parent.
fn open_rings(tree: &[RadialNode], open_path: &[usize], bands: RingBands, min_item_arc: f64) -> Vec<ArcRing> {
    let (inner, outer) = bands.band(0);
    let mut rings = vec![ArcRing::full(tree.len(), inner, outer)];
    let mut nodes = tree;
    for (k, &i) in open_path.iter().enumerate() {
        let Some(node) = nodes.get(i) else { break };
        if node.children.is_empty() || k + 1 >= MAX_DEPTH {
            break;
        }
        let (inner, outer) = bands.band(k + 1);
        let ring = ArcRing::child(&rings[k], i, node.children.len(), inner, outer, min_item_arc);
        rings.push(ring);
        nodes = &node.children;
    }
    rings
}

/// The choice a pointer `dx` right of and `dy` BELOW the centre is aimed at,
/// as `(depth, index)`. `rings[0]` is ring 0 and `rings[k]` the open ring at
/// depth k.
///
/// Inside the hub nothing. Otherwise the deepest ring whose arc holds the
/// direction wins, once the pointer is past the ring beneath it: an outer
/// ring owns the gap before it and everything outward, within its arc, so a
/// flick that overshoots by a hundred points still picks what a careful one
/// does. A direction outside every outer arc falls back to ring 0, which
/// owns every direction from the hub out.
pub fn pick(rings: &[ArcRing], hub: f64, dx: f64, dy: f64) -> Option<(usize, usize)> {
    let first = rings.first()?;
    let r = dx.hypot(dy);
    if r < hub {
        return None;
    }
    let a = dx.atan2(-dy).rem_euclid(TAU);
    for k in (1..rings.len()).rev() {
        if r >= rings[k - 1].outer {
            if let Some(i) = rings[k].angle_index(a) {
                return Some((k, i));
            }
        }
    }
    first.angle_index(a).map(|i| (0, i))
}

/// The box around an annular sector from `a0` clockwise to `a1`, padded for
/// the feather. Equal ends mean the whole ring, which is what a ring of one
/// draws.
///
/// Each wedge is drawn in its own tight box rather than a quad over the
/// whole disc: in the frosted look every covered pixel samples the window,
/// and a disc-sized quad per wedge would sample it six times over for
/// nothing.
pub fn sector_bounds(centre: DVec2, inner: f64, outer: f64, a0: f64, a1: f64) -> Rect {
    let span = (a1 - a0).rem_euclid(TAU);
    let full = span == 0.0;
    let point = |a: f64, r: f64| dvec2(a.sin() * r, -a.cos() * r);
    let mut lo = dvec2(f64::MAX, f64::MAX);
    let mut hi = dvec2(f64::MIN, f64::MIN);
    let mut add = |p: DVec2| {
        lo = dvec2(lo.x.min(p.x), lo.y.min(p.y));
        hi = dvec2(hi.x.max(p.x), hi.y.max(p.y));
    };
    for a in [a0, a1] {
        add(point(a, inner));
        add(point(a, outer));
    }
    // A sector that crosses up, right, down or left bulges past its corners
    // there, and a box from the corners alone would clip the bulge.
    for quarter in 0..4 {
        let compass = quarter as f64 * PI * 0.5;
        if full || (compass - a0).rem_euclid(TAU) <= span {
            add(point(compass, outer));
        }
    }
    Rect {
        pos: dvec2(centre.x + lo.x - WEDGE_PAD, centre.y + lo.y - WEDGE_PAD),
        size: dvec2(hi.x - lo.x + WEDGE_PAD * 2.0, hi.y - lo.y + WEDGE_PAD * 2.0),
    }
}

/// What the rings do about where the pointer is now.
#[derive(Clone, Copy, Debug, PartialEq)]
enum RingChange {
    /// Nothing changes, and any dwell stops.
    Keep,
    /// Keep the first `n` open rings and close the rest.
    Truncate(usize),
    /// Open the ring of the parent at `(depth, index)` now.
    Open { depth: usize, index: usize },
    /// Close what lies more than one ring out, then wait on `(depth,
    /// index)`: a parent opens its ring when the wait ends, and a leaf
    /// closes the stale ring beside it.
    Dwell { depth: usize, index: usize },
}

/// The ring rule, as a pure function of the open path and what the pointer
/// is over. `openable` says the item under the pointer is an enabled parent
/// whose ring would still fit; `dwell_on` that resting may open anything.
///
/// Inward is back: the hub closes everything, and a band closes what lies
/// more than one ring outward of it, at once and without a delay, because
/// moving back toward the middle is how a hand says "not that". Outward is
/// slower to commit: crossing a parent's outer edge opens its ring at once,
/// since the hand is plainly heading there, but resting on it only opens
/// after the delay, so a pointer passing over a parent on its way elsewhere
/// does not throw rings open under itself.
fn ring_change(
    open_path: &[usize],
    hit: Option<(usize, usize)>,
    past_outer: bool,
    openable: bool,
    dwell_on: bool,
) -> RingChange {
    let Some((depth, index)) = hit else {
        return RingChange::Truncate(0);
    };
    let deeper = open_path.len() > depth + 1;
    let settle = if deeper { RingChange::Truncate(depth + 1) } else { RingChange::Keep };
    if openable {
        if open_path.get(depth) == Some(&index) {
            return settle;
        }
        if past_outer {
            return RingChange::Open { depth, index };
        }
        if dwell_on {
            return RingChange::Dwell { depth, index };
        }
        return settle;
    }
    // A leaf with a sibling's ring still open beyond it. That ring is
    // closed after the delay rather than at once, so a pointer that grazes
    // a neighbour on its way into the open ring does not shut it.
    if open_path.len() > depth && dwell_on {
        return RingChange::Dwell { depth, index };
    }
    settle
}

/// The choice a digit names on the focus ring, which is the deepest open
/// one: the digits count from one on that ring, so the numbers drawn and the
/// keys that answer move outward together.
fn digit_target(open_path: &[usize], code: KeyCode, count: usize) -> Option<(usize, usize)> {
    let n = key_number(code)?;
    (n <= count).then_some((open_path.len(), n - 1))
}

/// Whether a release may pick or open anything. The press that opened the
/// menu only counts once it has travelled a hub's width: the centre may have
/// been nudged in from an edge, and without the guard a press near that edge
/// would pick whatever wedge the nudge put under it.
fn release_counts(opening_press: bool, press_at: DVec2, at: DVec2, hub: f64) -> bool {
    !opening_press || (at - press_at).length() >= hub
}

/// Whether a press lands off the menu: past the outermost open ring by more
/// than the slop, which keeps a slightly wide press on a rim from counting
/// as a dismissal.
fn press_is_outside(r: f64, reach: f64, slop: f64) -> bool {
    r > reach + slop
}

/// Where the rings may go in a pass of `pass` points: inside the safe area
/// and a further `EDGE` in.
fn fit_bounds(pass: DVec2, left: f64, top: f64, right: f64, bottom: f64) -> Rect {
    Rect {
        pos: dvec2(left + EDGE, top + EDGE),
        size: dvec2(
            (pass.x - left - right - EDGE * 2.0).max(0.0),
            (pass.y - top - bottom - EDGE * 2.0).max(0.0),
        ),
    }
}

/// The share of its enter time a ring has had, from 0 to 1. Reduced motion
/// and a zero time both land it at once.
fn grow_share(elapsed: f64, secs: f64, reduced_motion: bool) -> f64 {
    if reduced_motion || secs <= 0.0 {
        1.0
    } else {
        (elapsed / secs).clamp(0.0, 1.0)
    }
}

/// How far a ring reaches toward its outer edge after `elapsed` of `secs` on
/// `ease`: 0 at its inner edge, 1 at rest. A spring goes past 1 on the way.
/// Exactly 1 once the time is up, whatever the curve's own last value, so a
/// ring always settles on the edge the pick reads.
fn grow_at(elapsed: f64, secs: f64, reduced_motion: bool, ease: &Ease) -> f64 {
    let share = grow_share(elapsed, secs, reduced_motion);
    if share >= 1.0 {
        1.0
    } else {
        ease.map(share)
    }
}

/// The outer edge a ring is drawn to at `grow`. Only the drawing reads it:
/// the pick, the digits and the outside test use the resting edge, so a ring
/// swinging past its edge never widens what a pointer reaches, and a ring
/// still on its way out can already be picked from. A curve that dips under
/// its start stops at the inner edge rather than turning the ring inside out.
fn drawn_outer(inner: f64, outer: f64, grow: f64) -> f64 {
    inner + (outer - inner) * grow.max(0.0)
}

/// The furthest `ease` carries a ring on its way from 0 to 1, never less
/// than 1: a spring's swing past its edge, and exactly 1 for a curve that
/// only settles.
fn ease_peak(ease: &Ease) -> f64 {
    (0..=PEAK_SAMPLES)
        .map(|i| ease.map(i as f64 / PEAK_SAMPLES as f64))
        .fold(1.0, f64::max)
}

/// How far from the centre the rings of a tree `levels` deep are ever
/// drawn: the resting reach of the deepest ring, or further where a ring's
/// curve swings past its edge. Ring 0 swings on `peak0`, every outer ring on
/// `peak_outer`. The centre is fitted against this, so a spring near a
/// window edge swings inside the window instead of being cut off by it.
fn fit_reach(bands: RingBands, levels: usize, peak0: f64, peak_outer: f64) -> f64 {
    (0..levels.max(1))
        .map(|depth| {
            let (inner, outer) = bands.band(depth);
            drawn_outer(inner, outer, if depth == 0 { peak0 } else { peak_outer })
        })
        .fold(0.0, f64::max)
}

/// The box a word or glyph `width` wide in a `size`-point face is drawn in,
/// centred on its ink at `at`: the font size tall, which holds the capitals
/// and the most of a descender a short word has.
fn word_box(at: DVec2, width: f64, size: f64) -> Rect {
    Rect {
        pos: dvec2(at.x - width * 0.5, at.y - size * 0.5),
        size: dvec2(width, size),
    }
}

/// Whether two boxes come within `clear` points of each other.
fn boxes_touch(a: &Rect, b: &Rect, clear: f64) -> bool {
    a.pos.x < b.pos.x + b.size.x + clear
        && b.pos.x < a.pos.x + a.size.x + clear
        && a.pos.y < b.pos.y + b.size.y + clear
        && b.pos.y < a.pos.y + a.size.y + clear
}

/// Whether `rect`, measured from the centre, lies inside wedge `i` of `ring`
/// with `clear` points to spare from both arcs and both sides, the drawn
/// seam of `half_gap` either side included.
///
/// The corners decide the outer arc and the sides, which bound a convex
/// region for any wedge up to half a circle; the inner arc curves the other
/// way, so there the point of the box nearest the centre decides.
fn wedge_holds(ring: &ArcRing, i: usize, half_gap: f64, rect: &Rect, clear: f64) -> bool {
    let (x0, y0) = (rect.pos.x, rect.pos.y);
    let (x1, y1) = (x0 + rect.size.x, y0 + rect.size.y);
    let nearest = dvec2(0.0f64.clamp(x0, x1), 0.0f64.clamp(y0, y1));
    if nearest.length() < ring.inner + clear {
        return false;
    }
    let whole = ring.count <= 1 && ring.span >= TAU;
    let from = ring.start + i as f64 * ring.step() + half_gap;
    let span = ring.step() - 2.0 * half_gap;
    [dvec2(x0, y0), dvec2(x1, y0), dvec2(x0, y1), dvec2(x1, y1)].iter().all(|p| {
        let r = p.length();
        if r > ring.outer - clear {
            return false;
        }
        if whole {
            return true;
        }
        let t = (p.x.atan2(-p.y) - from).rem_euclid(TAU);
        t <= span && t * r >= clear && (span - t) * r >= clear
    })
}

/// Where wedge `i`'s number goes, as an offset from the centre, for a number
/// `size` big (its measured width and its font size) among `words`, the
/// boxes of every word and glyph drawn, measured from the centre too.
///
/// The number starts at `NUMBER_AT` on the wedge's middle line and takes the
/// nearest spot, counting points moved in and out plus points moved along
/// the arc, that keeps `NUMBER_CLEAR` from every word and inside its wedge.
/// Of two spots as near, the one further in wins, then the one higher on
/// screen, then the one further left, so the choice never flickers between
/// frames. A ring too crowded for any spot keeps the starting one.
fn number_spot(ring: &ArcRing, i: usize, half_gap: f64, size: DVec2, words: &[Rect]) -> DVec2 {
    let c = ring.centre(i);
    let point = |r: f64, s: f64| {
        let a = c + s / r.max(1.0);
        dvec2(a.sin() * r, -a.cos() * r)
    };
    let fits = |p: DVec2| {
        let rect = word_box(p, size.x, size.y);
        !words.iter().any(|word| boxes_touch(&rect, word, NUMBER_CLEAR))
            && wedge_holds(ring, i, half_gap, &rect, NUMBER_CLEAR)
    };
    let start = ring.inner + (ring.outer - ring.inner) * NUMBER_AT;
    let inward = (start - ring.inner).max(0.0).floor() as usize;
    let outward = (ring.outer - start).max(0.0).floor() as usize;
    // Half the widest arc the wedge has, at its outer edge; no spot further
    // along than that can be inside it.
    let along = ((ring.step() * 0.5 * ring.outer).min(240.0)).ceil() as usize;
    let whole = ring.count <= 1 && ring.span >= TAU;
    let half_angle = (ring.step() * 0.5 - half_gap).max(0.0);
    for cost in 0..=(inward.max(outward) + along) {
        for moved in 0..=cost {
            let s = (cost - moved) as f64;
            if s as usize > along {
                continue;
            }
            for r in [start - moved as f64, start + moved as f64] {
                if (r < start && moved > inward) || (r > start && moved > outward) {
                    continue;
                }
                // Past the wedge's own side at this radius: no box centred
                // there is inside it, so the full test is not worth making.
                if !whole && s > half_angle * r {
                    if moved == 0 {
                        break;
                    }
                    continue;
                }
                let a = point(r, s);
                let b = point(r, -s);
                let (first, second) = if (a.y, a.x) <= (b.y, b.x) { (a, b) } else { (b, a) };
                if fits(first) {
                    return first;
                }
                if s > 0.0 && fits(second) {
                    return second;
                }
                if moved == 0 {
                    break;
                }
            }
        }
    }
    point(start, 0.0)
}

/// The state a test reads in one line: where the centre is, which ring the
/// digits act on, what is hot, and which parents have their rings open.
fn format_snapshot(open: Option<(DVec2, usize, Option<&str>, &[String])>) -> String {
    match open {
        None => "closed".to_string(),
        Some((at, focus, hot, rings)) => format!(
            "open at={},{} focus={} hot={} rings={}",
            at.x.round() as i64,
            at.y.round() as i64,
            focus,
            hot.unwrap_or("-"),
            rings.join("|")
        ),
    }
}

/// What opens the menu.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum RadialTrigger {
    /// A primary press in the field.
    #[pick]
    Press = 0,
    /// A secondary press or a long touch in the field; a primary press goes
    /// through to what is underneath.
    Secondary = 1,
    /// Only `open_at`.
    Manual = 2,
}

/// How the wedges are filled.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum RadialLook {
    /// Theme fills.
    #[pick]
    Solid = 0,
    /// The window behind, blurred and tinted toward each wedge's role.
    Frosted = 1,
}

/// What the menu reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RadialPick {
    pub id: LiveId,
    pub key: String,
    /// The index at each depth: `path[k]` is the choice on ring k.
    pub path: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum RadialAction {
    Opened,
    /// The parent whose outer ring just opened.
    RingOpened(LiveId),
    Picked(RadialPick),
    /// Escape, an outside press, a release in the dead zone, or lost focus.
    Cancelled,
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*

    // Not splatted: `Press`, `Manual` and `Solid` are words a widget could
    // plausibly want bare. Written out as `RadialTrigger.Secondary`, the way
    // PanelEdge is, they cannot shadow anything.
    mod.widgets.RadialLook = set_type_default() do #(RadialLook::script_api(vm))
    mod.widgets.RadialTrigger = set_type_default() do #(RadialTrigger::script_api(vm))

    /** One choice: its full key path ("share/mail"), its label, an optional
     * glyph, and whether it can be picked. */
    mod.widgets.RadialItem = #(RadialItem::script_api(vm))

    use mod.widgets.*

    mod.widgets.DrawRadialWedgeBase = #(DrawRadialWedge::script_component(vm))
    set_type_default() do #(DrawRadialWedge::script_shader(vm)){
        ..mod.draw.DrawQuad

        // Rust writes one of these per wedge, so each is a plain literal.
        centre: vec2(0.0, 0.0)
        a0: 0.0
        a1: 0.0
        inner: 30.0
        outer: 96.0
        grow: 1.0
        hot: 0.0
        open: 0.0
        disabled: 0.0
        has_children: 0.0
        frosted: 0.0
        hub: 0.0

        /** the face of a wedge the pointer is not aimed at */
        color_wedge: uniform(theme.color_surface_container_high)
        /** the face of the wedge the pointer is aimed at */
        color_wedge_hot: uniform(theme.color_primary)
        /** the face of the parent whose ring is open */
        color_wedge_open: uniform(theme.color_secondary_container)
        /** the band along that parent's rim, which marks the way out to its ring in every theme */
        color_wedge_open_rim: uniform(theme.color_secondary)
        /** the face of the dead zone */
        color_hub: uniform(theme.color_surface_container)
        /** the hub's edge and the mark on a choice that opens a ring */
        color_edge: uniform(theme.color_outline)
        /** that mark over the hot wedge, in the hot label's colour */
        color_tick_hot: uniform(theme.color_on_primary)
        /** the hairline along a frosted ring's arcs */
        color_rim: uniform(theme.color_outline_variant)
        /** the frosted face before there is a window to sample */
        fallback_color: uniform(mix(theme.color_bg_app, theme.color_text, 0.30))
        /** how strongly a disabled choice's words show */
        disabled_alpha: uniform(theme.state_disabled_content_opacity)
        // Copied from the widget on every draw.
        frost_level: uniform(2.5)
        frost_tint: uniform(0.55)

        // Bound in this order by `bind_gauss_snapshot`: the scene, then the
        // pyramid.
        scene_texture: texture_2d(float)
        mip0_texture: texture_2d(float)
        mip1_texture: texture_2d(float)
        mip2_texture: texture_2d(float)
        mip3_texture: texture_2d(float)
        mip4_texture: texture_2d(float)
        mip5_texture: texture_2d(float)
        has_gauss: uniform(0.0)
        source_size: uniform(vec2(1.0, 1.0))
        source_y_flip: uniform(0.0)

        // The glass family's pyramid sampling, copied rather than inherited:
        // this quad is a wedge and not a rounded rect, and inheriting the
        // glass surface would mean replacing its whole pixel function. The
        // B-spline taps keep a low mip stretched over a wedge from showing
        // its texel grid.
        bicubic_h: fn(uv: vec2, size: vec2) -> vec4 {
            let tc = uv * size - 0.5
            let f = fract(tc)
            let tc0 = floor(tc)
            let f2 = f * f
            let f3 = f2 * f
            let omf = 1.0 - f
            let w1 = (f3 * 3.0 - f2 * 6.0 + 4.0) / 6.0
            let g0 = omf * omf * omf / 6.0 + w1
            let h0 = clamp((tc0 - 0.5 + w1 / g0) / size, vec2(0.0, 0.0), vec2(1.0, 1.0))
            let h1 = clamp((tc0 + 1.5 + (f3 / 6.0) / (1.0 - g0)) / size, vec2(0.0, 0.0), vec2(1.0, 1.0))
            return vec4(h0.x, h0.y, h1.x, h1.y)
        }

        bicubic_g0: fn(uv: vec2, size: vec2) -> vec2 {
            let f = fract(uv * size - 0.5)
            let f2 = f * f
            let omf = 1.0 - f
            return omf * omf * omf / 6.0 + (f2 * f * 3.0 - f2 * 6.0 + 4.0) / 6.0
        }

        sample_level: fn(level: float, uv: vec2) -> vec4 {
            let source_uv = vec2(uv.x, mix(uv.y, 1.0 - uv.y, self.source_y_flip))
            let safe_uv = clamp(source_uv, vec2(0.0, 0.0), vec2(1.0, 1.0))
            if level < 0.5 {
                return self.scene_texture.sample_as_bgra(safe_uv)
            }
            if level < 1.5 {
                let size = max(self.mip0_texture.size(), vec2(1.0, 1.0))
                let h = self.bicubic_h(safe_uv, size)
                let g0 = self.bicubic_g0(safe_uv, size)
                let g1 = 1.0 - g0
                return self.mip0_texture.sample_as_bgra(vec2(h.x, h.y)) * (g0.x * g0.y)
                    + self.mip0_texture.sample_as_bgra(vec2(h.z, h.y)) * (g1.x * g0.y)
                    + self.mip0_texture.sample_as_bgra(vec2(h.x, h.w)) * (g0.x * g1.y)
                    + self.mip0_texture.sample_as_bgra(vec2(h.z, h.w)) * (g1.x * g1.y)
            }
            if level < 2.5 {
                let size = max(self.mip1_texture.size(), vec2(1.0, 1.0))
                let h = self.bicubic_h(safe_uv, size)
                let g0 = self.bicubic_g0(safe_uv, size)
                let g1 = 1.0 - g0
                return self.mip1_texture.sample_as_bgra(vec2(h.x, h.y)) * (g0.x * g0.y)
                    + self.mip1_texture.sample_as_bgra(vec2(h.z, h.y)) * (g1.x * g0.y)
                    + self.mip1_texture.sample_as_bgra(vec2(h.x, h.w)) * (g0.x * g1.y)
                    + self.mip1_texture.sample_as_bgra(vec2(h.z, h.w)) * (g1.x * g1.y)
            }
            if level < 3.5 {
                let size = max(self.mip2_texture.size(), vec2(1.0, 1.0))
                let h = self.bicubic_h(safe_uv, size)
                let g0 = self.bicubic_g0(safe_uv, size)
                let g1 = 1.0 - g0
                return self.mip2_texture.sample_as_bgra(vec2(h.x, h.y)) * (g0.x * g0.y)
                    + self.mip2_texture.sample_as_bgra(vec2(h.z, h.y)) * (g1.x * g0.y)
                    + self.mip2_texture.sample_as_bgra(vec2(h.x, h.w)) * (g0.x * g1.y)
                    + self.mip2_texture.sample_as_bgra(vec2(h.z, h.w)) * (g1.x * g1.y)
            }
            if level < 4.5 {
                let size = max(self.mip3_texture.size(), vec2(1.0, 1.0))
                let h = self.bicubic_h(safe_uv, size)
                let g0 = self.bicubic_g0(safe_uv, size)
                let g1 = 1.0 - g0
                return self.mip3_texture.sample_as_bgra(vec2(h.x, h.y)) * (g0.x * g0.y)
                    + self.mip3_texture.sample_as_bgra(vec2(h.z, h.y)) * (g1.x * g0.y)
                    + self.mip3_texture.sample_as_bgra(vec2(h.x, h.w)) * (g0.x * g1.y)
                    + self.mip3_texture.sample_as_bgra(vec2(h.z, h.w)) * (g1.x * g1.y)
            }
            if level < 5.5 {
                let size = max(self.mip4_texture.size(), vec2(1.0, 1.0))
                let h = self.bicubic_h(safe_uv, size)
                let g0 = self.bicubic_g0(safe_uv, size)
                let g1 = 1.0 - g0
                return self.mip4_texture.sample_as_bgra(vec2(h.x, h.y)) * (g0.x * g0.y)
                    + self.mip4_texture.sample_as_bgra(vec2(h.z, h.y)) * (g1.x * g0.y)
                    + self.mip4_texture.sample_as_bgra(vec2(h.x, h.w)) * (g0.x * g1.y)
                    + self.mip4_texture.sample_as_bgra(vec2(h.z, h.w)) * (g1.x * g1.y)
            }
            let size = max(self.mip5_texture.size(), vec2(1.0, 1.0))
            let h = self.bicubic_h(safe_uv, size)
            let g0 = self.bicubic_g0(safe_uv, size)
            let g1 = 1.0 - g0
            return self.mip5_texture.sample_as_bgra(vec2(h.x, h.y)) * (g0.x * g0.y)
                + self.mip5_texture.sample_as_bgra(vec2(h.z, h.y)) * (g1.x * g0.y)
                + self.mip5_texture.sample_as_bgra(vec2(h.x, h.w)) * (g0.x * g1.y)
                + self.mip5_texture.sample_as_bgra(vec2(h.z, h.w)) * (g1.x * g1.y)
        }

        sample_blur: fn(level: float, uv: vec2) -> vec4 {
            let safe_level = clamp(level, 0.0, 6.0)
            if safe_level >= 5.999 {
                return self.sample_level(6.0, uv)
            }
            let base_level = floor(safe_level)
            let t = safe_level - base_level
            let l1 = base_level
            let l2 = min(base_level + 1.0, 6.0)
            let blend = t * t * (3.0 - 2.0 * t)
            let c1 = self.sample_level(l1, uv)
            let c2 = self.sample_level(l2, uv)
            return c1.mix(c2, blend)
        }

        // The window at `q`, blurred, with `amount` of a role colour laid
        // over it. Before a capture exists the flat fallback stands in for
        // the window, so the face is never see-through.
        frost: fn(q: vec2, col: vec3, amount: float) -> vec3 {
            let sampled = self.sample_blur(self.frost_level, q)
            let transmitted = mix(self.fallback_color.xyz, sampled.xyz, self.has_gauss)
            return mix(transmitted, col, clamp(amount, 0.0, 1.0))
        }

        pixel: fn() {
            let local = self.pos * self.rect_size
            let p = local - self.centre
            let r = length(p)
            let screen_pos = self.rect_pos + local
            let q = clamp(screen_pos / max(self.source_size, vec2(1.0, 1.0)), vec2(0.0, 0.0), vec2(1.0, 1.0))

            if self.hub > 0.5 {
                // The hub keeps its size while ring 0 grows and only fades
                // in, so the dead zone is the right size from the first
                // frame, as the pie menu's is.
                if r > self.outer + 1.5 {
                    return vec4(0.0, 0.0, 0.0, 0.0)
                }
                let sdf = Sdf2d.viewport(local)
                sdf.circle(self.centre.x, self.centre.y, self.outer)
                let mut face = self.color_hub
                if self.frosted > 0.5 {
                    face = vec4(self.frost(q, self.color_hub.xyz, self.frost_tint), 1.0)
                }
                sdf.fill_keep(face)
                sdf.stroke(self.color_edge, 1.0)
                // Held to 1: a spring's swing is movement, and the hub is
                // no brighter for it.
                return sdf.result * clamp(self.grow, 0.0, 1.0)
            }

            // The ring grows outward from its inner edge, so the hole is the
            // right size from the first frame and only the reach animates.
            // `drawn_outer` in Rust is the same sum, and sizes the box.
            let outer = self.inner + (self.outer - self.inner) * max(self.grow, 0.0)
            if r < self.inner || r > outer {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let mut a = atan2(p.x, 0.0 - p.y)
            if a < 0.0 {
                a = a + 2.0 * PI
            }
            let mut inside = 0.0
            if self.a1 > self.a0 {
                if a >= self.a0 && a <= self.a1 { inside = 1.0 }
            } else {
                // The wedge that straddles twelve o'clock: its start is the
                // larger number, so it is two arcs and not one.
                if a >= self.a0 || a <= self.a1 { inside = 1.0 }
            }
            if inside < 0.5 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let mut d0 = a - self.a0
            if d0 < 0.0 { d0 = d0 + 2.0 * PI }
            let mut d1 = self.a1 - a
            if d1 < 0.0 { d1 = d1 + 2.0 * PI }
            // The sides are as far away in points as the arc they cut, so
            // one distance feathers all four edges the same.
            let edge = min(min(r - self.inner, outer - r), min(d0, d1) * r)
            let aa = clamp(edge, 0.0, 1.5) / 1.5

            let col = self.color_wedge.mix(self.color_wedge_open, self.open).mix(self.color_wedge_hot, self.hot)
            let mut rgb = col.xyz
            let mut alpha = col.w
            if self.frosted > 0.5 {
                rgb = self.frost(q, col.xyz, self.frost_tint + 0.25 * self.hot)
                // A hairline along both arcs, where one frosted ring stops
                // and the window starts again; without it neighbouring rings
                // run together wherever the window behind is plain.
                let rim = clamp(1.0 - min(abs(r - self.inner), abs(outer - r)), 0.0, 1.0)
                rgb = mix(rgb, self.color_rim.xyz, self.color_rim.w * rim)
                // Opaque: a surface drawn over a captured window replaces
                // what is behind it, or the scene shows through twice.
                alpha = 1.0
            }
            rgb = mix(rgb, self.color_wedge.xyz, self.disabled * 0.5)

            if self.has_children > 0.5 {
                // A short arc just inside the rim says "there is more out
                // this way", which is the only hint a ring can give before
                // the hand gets there.
                let mut span = self.a1 - self.a0
                if span < 0.0 { span = span + 2.0 * PI }
                let da = d0 - span * 0.5
                let half = 6.0 * PI / 180.0
                let tick_r = outer - 5.0
                let tick = clamp(1.5 - abs(r - tick_r), 0.0, 1.0) * clamp((half - abs(da)) * r + 0.5, 0.0, 1.0)
                let tick_col = self.color_edge.mix(self.color_tick_hot, self.hot)
                rgb = mix(rgb, tick_col.xyz, tick * tick_col.w)
            }
            // The parent whose ring is open carries a band 3 points deep
            // along its rim, toward the ring it opened. Its face alone is not
            // enough: a theme may give the open face and the resting face all
            // but the same grey, and then nothing on ring 0 says which parent
            // the outer ring belongs to.
            let band = clamp(3.5 - (outer - r), 0.0, 1.0) * self.open
            rgb = mix(rgb, self.color_wedge_open_rim.xyz, band * self.color_wedge_open_rim.w)
            // Premultiplied, like everything an Sdf2d returns: this shader
            // builds its colour by hand, so it has to do what fill() would
            // have done, or the feathered edges come back too bright.
            // The fade is the reach held to 0..1: a ring past its edge is no
            // more opaque than one at rest.
            return Pal.premul(vec4(rgb, alpha * aa * clamp(self.grow, 0.0, 1.0)))
        }
    }

    mod.widgets.DrawRadialFieldBase = #(DrawRadialField::script_component(vm))
    set_type_default() do #(DrawRadialField::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            // Nothing: the field is where a press may open the menu, not a
            // surface. The rings are on the overlay and leave no rect here,
            // so without this quad the widget has nothing to be pressed on,
            // to redraw, or to hold the keyboard with.
            return vec4(0.0, 0.0, 0.0, 0.0)
        }
    }

    mod.widgets.RadialMenuBase = #(RadialMenu::register_widget(vm))

    /** A ring of choices around a point; a choice with children opens an
     * outer ring in its own direction, so one flick outward picks a choice
     * two or three rings deep. */
    mod.widgets.RadialMenu = set_type_default() do mod.widgets.RadialMenuBase{
        // The field: where a press can open the menu. A Fit field is 0 by 0,
        // which suits a menu opened only from Rust.
        width: Fill
        height: Fill
        /** the choices, flat: RadialItem{key: "share/mail" label: "Mail"} */
        items: []
        /** what opens it: RadialTrigger.Press Secondary Manual */
        trigger: RadialTrigger.Press
        /** theme fills, or the window blurred behind: RadialLook.Solid Frosted */
        look: RadialLook.Solid
        /** the outer edge of the first ring 40..200 step 2 */
        radius: 96.
        /** the hole in the middle, which is also the dead zone 10..80 step 2 */
        hub_radius: 30.
        /** how deep each outer ring is 32..120 step 2 */
        ring_width: 64.
        /** the space between two rings 0..16 step 1 */
        ring_gap: 4.
        /** the gap drawn between two wedges, in degrees 0..12 step 0.5 */
        gap: 2.
        /** the least arc, in points, an outer choice gets before its ring widens 24..96 step 2 */
        min_item_arc: 44.
        /** how long resting on a parent takes to open its ring, in seconds; 0 turns resting off 0..1 step 0.05 */
        open_delay: 0.3
        /** how far past the outermost ring a press still counts as on the menu 0..96 step 2 */
        outside_slop: 24.
        /** number the first nine choices of the outermost open ring 0..1 step 1 */
        show_numbers: true
        /** how long the first ring takes to grow open, in seconds 0..0.6 step 0.01 */
        enter_secs: theme.motion_short_3
        /** how long an outer ring takes to grow open, in seconds 0..0.6 step 0.01 */
        ring_enter_secs: theme.motion_short_2
        /** the curve every ring grows open on: one of the theme's motion_ease tokens */
        enter_ease: theme.motion_ease_emphasized_decelerate
        /** frosted: how blurred the window behind is, as a pyramid level 0..6 step 0.25 */
        frost_level: 2.5
        /** frosted: how much of each wedge's colour lies over the window 0..1 step 0.05 */
        frost_tint: 0.55
        /** cut every grow to its end, for readers who asked for less movement 0..1 step 1 */
        reduced_motion: false

        // Plain values only: Rust writes all of these per wedge, and the
        // colours and the glass inputs are the shader's own uniforms.
        draw_wedge +: {
            centre: vec2(0.0, 0.0)
            a0: 0.0
            a1: 0.0
            inner: 30.0
            outer: 96.0
            grow: 1.0
            hot: 0.0
            open: 0.0
            disabled: 0.0
            has_children: 0.0
            frosted: 0.0
            hub: 0.0
        }
        draw_label +: {
            color: theme.color_text
            // 1.0, because the label is centred on the wedge by arithmetic
            // that reads the font size as the line's whole height.
            text_style: theme.font_regular{font_size: theme.font_size_p line_spacing: 1.0}
        }
        draw_label_hot +: {
            color: theme.color_on_primary
            text_style: theme.font_regular{font_size: theme.font_size_p line_spacing: 1.0}
        }
        draw_label_open +: {
            color: theme.color_on_secondary_container
            text_style: theme.font_regular{font_size: theme.font_size_p line_spacing: 1.0}
        }
        draw_number +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.type_label_s_size line_spacing: 1.0}
        }
        draw_icon +: {
            color: theme.color_text
            text_style: theme.font_icons{font_size: 14. line_spacing: 1.0}
        }
    }

    /** The rings over whatever is behind them, blurred and tinted. */
    mod.widgets.RadialMenuFrosted = mod.widgets.RadialMenu{
        look: RadialLook.Frosted
    }

    /** Opened by a secondary press or a long touch; a primary press goes
     * through to what is underneath. */
    mod.widgets.RadialMenuContext = mod.widgets.RadialMenu{
        trigger: RadialTrigger.Secondary
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawRadialWedge {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    centre: Vec2f,
    #[live]
    a0: f32,
    #[live]
    a1: f32,
    #[live]
    inner: f32,
    #[live]
    outer: f32,
    #[live]
    grow: f32,
    #[live]
    hot: f32,
    #[live]
    open: f32,
    #[live]
    disabled: f32,
    #[live]
    has_children: f32,
    #[live]
    frosted: f32,
    #[live]
    hub: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawRadialField {
    #[deref]
    draw_super: DrawQuad,
}

/// A menu that is up.
#[derive(Clone, Debug)]
struct OpenState {
    centre: DVec2,
    /// False while the centre is still the point asked for, because the
    /// window's size was not known yet; the next draw fits it.
    fitted: bool,
    opened_at: f64,
    press_at: DVec2,
    /// True from the press that opened the menu until that press is
    /// released. Its release lands in the dead zone, because that is where
    /// the menu appeared around it, and without this it would read as
    /// "released on nothing: cancel" and a click could never open a menu.
    opening_press: bool,
    /// The parent at each depth whose ring is open.
    open_path: Vec<usize>,
    /// When the ring at each depth opened; `[0]` is the menu itself.
    ring_opened_at: [f64; MAX_DEPTH],
    /// The enabled choice the pointer is aimed at.
    hot: Option<(usize, usize)>,
    /// The choice the pointer is aimed at, enabled or not: a dwell waits on
    /// this, so resting on a disabled leaf still closes a stale ring.
    aim: Option<(usize, usize)>,
    dwell: Option<(usize, usize)>,
    pointer: Option<DVec2>,
}

#[derive(Script, Widget)]
pub struct RadialMenu {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    /// The field's own rect, drawn as nothing — see the shader's comment.
    #[redraw]
    #[live]
    draw_field: DrawRadialField,
    #[live]
    pub draw_wedge: DrawRadialWedge,
    #[live]
    pub draw_label: DrawText,
    #[live]
    pub draw_label_hot: DrawText,
    #[live]
    pub draw_label_open: DrawText,
    #[live]
    pub draw_number: DrawText,
    #[live]
    pub draw_icon: DrawText,

    #[live]
    pub items: Vec<RadialItem>,
    #[live]
    pub trigger: RadialTrigger,
    #[live]
    pub look: RadialLook,
    #[live(96.0)]
    pub radius: f64,
    #[live(30.0)]
    pub hub_radius: f64,
    #[live(64.0)]
    pub ring_width: f64,
    #[live(4.0)]
    pub ring_gap: f64,
    #[live(2.0)]
    pub gap: f64,
    #[live(44.0)]
    pub min_item_arc: f64,
    #[live(0.3)]
    pub open_delay: f64,
    #[live(24.0)]
    pub outside_slop: f64,
    #[live(true)]
    pub show_numbers: bool,
    #[live(0.15)]
    pub enter_secs: f64,
    #[live(0.1)]
    pub ring_enter_secs: f64,
    #[live(Ease::Bezier { cp0: 0.05, cp1: 0.7, cp2: 0.1, cp3: 1.0 })]
    pub enter_ease: Ease,
    #[live(2.5)]
    pub frost_level: f64,
    #[live(0.55)]
    pub frost_tint: f64,
    #[live(false)]
    pub reduced_motion: bool,

    #[rust]
    draw_list: Option<DrawList2d>,
    #[rust]
    tree: Vec<RadialNode>,
    /// `set_items` was called: the DSL list does not re-seed the tree until
    /// the template itself is reloaded.
    #[rust]
    items_from_rust: bool,
    #[rust]
    open: Option<OpenState>,
    #[rust]
    dwell_timer: Timer,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    locked: bool,
    /// Where the keyboard was when the menu opened, given back on close: the
    /// menu takes it for its digits, and a context menu over a text field
    /// must not leave the field without it.
    #[rust]
    restore_focus: Area,
    /// Opened before the field had an area to give the keyboard to; the
    /// first draw that gives it one does.
    #[rust]
    focus_owed: bool,
    /// A press outside closed the menu; its release is eaten so the
    /// dismissing click does not also act on what was under the menu.
    #[rust]
    swallow_up: bool,
    #[rust]
    field_area: Area,
    #[rust]
    pass_size: DVec2,
    /// The touch being followed. Only the first finger steers the rings.
    #[rust]
    touch: Option<u64>,
}

impl ScriptHook for RadialMenu {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }

    fn on_after_apply(&mut self, vm: &mut ScriptVm, apply: &Apply, _scope: &mut Scope, _value: ScriptValue) {
        // A reload of the template makes the DSL list the source of truth
        // again; any other apply (a control nudging the radius) must not
        // throw away items a caller set from Rust.
        if apply.is_template_apply() {
            self.items_from_rust = false;
        }
        let mut reset = false;
        if !self.items_from_rust {
            let flat: Vec<(&str, &str, &str, bool)> = self
                .items
                .iter()
                .map(|item| (item.key.as_str(), item.label.as_str(), item.icon.as_str(), item.enabled))
                .collect();
            let (tree, dropped) = build_tree(&flat);
            if apply.is_template_apply() {
                for key in dropped {
                    log!("RadialMenu: item \"{key}\" dropped: an empty segment, more than {MAX_DEPTH} rings deep, a parent declared after it or not at all, or a key used twice");
                }
            }
            if tree != self.tree {
                self.tree = tree;
                reset = true;
            }
        }
        vm.with_cx_mut(|cx| {
            if reset {
                self.reset_rings(cx);
            }
            if self.look == RadialLook::Frosted && self.open.is_some() {
                arm_gauss_capture(cx);
            }
            self.redraw_menu(cx);
        });
    }
}

impl RadialMenu {
    fn bands(&self) -> RingBands {
        RingBands::new(self.hub_radius, self.radius, self.ring_width, self.ring_gap)
    }

    fn rings_for(&self, open: &OpenState) -> Vec<ArcRing> {
        open_rings(&self.tree, &open.open_path, self.bands(), self.min_item_arc)
    }

    /// For each of the first `count` open rings, the share of its enter time
    /// gone and the reach the ease gives it at `now`. Ring 0 is timed from
    /// the menu opening, an outer ring from its own.
    fn grows(&self, open: &OpenState, count: usize, now: f64) -> Vec<(f64, f64)> {
        (0..count)
            .map(|depth| {
                let (secs, since) = if depth == 0 {
                    (self.enter_secs, open.opened_at)
                } else {
                    (self.ring_enter_secs, open.ring_opened_at[depth])
                };
                let elapsed = now - since;
                (
                    grow_share(elapsed, secs, self.reduced_motion),
                    grow_at(elapsed, secs, self.reduced_motion, &self.enter_ease),
                )
            })
            .collect()
    }

    fn node_at(&self, open_path: &[usize], depth: usize, index: usize) -> Option<&RadialNode> {
        ring_nodes(&self.tree, open_path, depth)?.get(index)
    }

    /// An enabled parent whose ring would still be within the depth limit.
    fn openable(node: &RadialNode, depth: usize) -> bool {
        node.enabled && !node.children.is_empty() && depth + 1 < MAX_DEPTH
    }

    fn redraw_menu(&mut self, cx: &mut Cx) {
        self.draw_field.redraw(cx);
        if let Some(list) = &self.draw_list {
            list.redraw(cx);
        }
    }

    fn lock(&mut self, cx: &mut Cx) {
        // An area that was never drawn is Empty, and a lock held by Empty
        // would turn away every hit test in the window.
        if !self.locked && !self.field_area.is_empty() {
            cx.sweep_lock(self.field_area);
            self.locked = true;
        }
    }

    fn unlock(&mut self, cx: &mut Cx) {
        if self.locked {
            cx.sweep_unlock(self.field_area);
            self.locked = false;
        }
    }

    fn stop_dwell(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.dwell_timer);
        self.dwell_timer = Timer::empty();
        if let Some(open) = self.open.as_mut() {
            open.dwell = None;
        }
    }

    fn keep_growing(&mut self, cx: &mut Cx) {
        if !self.reduced_motion {
            self.next_frame = cx.new_next_frame();
        }
    }

    /// The tree changed under an open menu: the path and the aim name
    /// choices that may no longer exist, so the menu drops back to ring 0.
    fn reset_rings(&mut self, cx: &mut Cx) {
        self.stop_dwell(cx);
        if let Some(open) = self.open.as_mut() {
            open.open_path.clear();
            open.hot = None;
            open.aim = None;
        }
    }

    /// Open the menu around `at`, in window coordinates.
    pub fn open_at(&mut self, cx: &mut Cx, at: DVec2) {
        self.open_with(cx, at, false);
    }

    fn open_with(&mut self, cx: &mut Cx, at: DVec2, from_press: bool) {
        if self.tree.is_empty() {
            return;
        }
        if self.open.is_none() {
            self.restore_focus = cx.key_focus();
        }
        self.stop_dwell(cx);
        // Fitted once, against the deepest ring the tree can open, so that
        // opening an outer ring never moves the rings already under the
        // pointer; and against the swing, so a spring near an edge stays in
        // the window.
        let full = self.fit_reach();
        let bounds = self.bounds(cx);
        let centre = bounds.map_or(at, |bounds| fit_centre(bounds, at, full));
        let now = cx.seconds_since_app_start();
        self.open = Some(OpenState {
            centre,
            fitted: bounds.is_some(),
            opened_at: now,
            press_at: at,
            opening_press: from_press,
            open_path: Vec::new(),
            ring_opened_at: [now; MAX_DEPTH],
            hot: None,
            aim: None,
            dwell: None,
            pointer: None,
        });
        self.lock(cx);
        self.focus_owed = true;
        self.take_focus(cx);
        if self.look == RadialLook::Frosted {
            // Before the next frame begins, or that frame paints the rings
            // without a capture and they show the fallback face for a frame.
            arm_gauss_capture(cx);
        }
        self.keep_growing(cx);
        self.redraw_menu(cx);
        let uid = self.uid;
        cx.widget_action(uid, RadialAction::Opened);
    }

    /// How far from the centre the tree's rings are ever drawn, swing and
    /// all, for the rings as they are timed now.
    fn fit_reach(&self) -> f64 {
        let peak = |secs: f64| {
            if self.reduced_motion || secs <= 0.0 {
                1.0
            } else {
                ease_peak(&self.enter_ease)
            }
        };
        fit_reach(self.bands(), levels(&self.tree), peak(self.enter_secs), peak(self.ring_enter_secs))
    }

    /// The window's usable rect, once a draw has told us the window's size.
    fn bounds(&self, cx: &Cx) -> Option<Rect> {
        if self.pass_size.x <= 0.0 || self.pass_size.y <= 0.0 {
            return None;
        }
        let insets = cx.display_context.safe_area_insets;
        Some(fit_bounds(self.pass_size, insets.left, insets.top, insets.right, insets.bottom))
    }

    /// Take the menu down without reporting anything.
    pub fn close(&mut self, cx: &mut Cx) {
        self.take_down(cx, false);
    }

    /// Every close funnels through here. `keep_lock` holds the pointer until
    /// the release of a dismissing press has been eaten.
    fn take_down(&mut self, cx: &mut Cx, keep_lock: bool) -> bool {
        self.stop_dwell(cx);
        let was_open = self.open.take().is_some();
        if !keep_lock {
            self.unlock(cx);
        }
        if was_open {
            self.touch = None;
            self.give_back_focus(cx);
            self.redraw_menu(cx);
        }
        was_open
    }

    /// Give the field the keyboard, once it has an area to give it to.
    fn take_focus(&mut self, cx: &mut Cx) {
        if self.focus_owed && !self.field_area.is_empty() {
            self.focus_owed = false;
            cx.set_key_focus(self.field_area);
        }
    }

    /// The keyboard goes back where it was when the menu opened, unless it
    /// has gone somewhere else since: a press that moved it on and closed
    /// the menu keeps the place it moved to. Still where it was means the
    /// menu opened and closed within one event, its move to the field not
    /// made yet, and asking again is what cancels that move.
    ///
    /// The place is followed to the handle it has now: the page under the
    /// rings is usually drawn again while they are up, and the handle kept
    /// at open then names nothing, so the keyboard given to it would reach
    /// no widget at all.
    fn give_back_focus(&mut self, cx: &mut Cx) {
        self.focus_owed = false;
        let restore = area_after_redraws(cx, std::mem::take(&mut self.restore_focus));
        let focus = cx.key_focus();
        let ours = !self.field_area.is_empty() && focus == self.field_area;
        if ours || focus == restore {
            cx.set_key_focus(restore);
        }
    }

    fn cancel(&mut self, cx: &mut Cx) {
        if self.take_down(cx, false) {
            let uid = self.uid;
            cx.widget_action(uid, RadialAction::Cancelled);
        }
    }

    fn choose(&mut self, cx: &mut Cx, pick: RadialPick) {
        self.take_down(cx, false);
        let uid = self.uid;
        cx.widget_action(uid, RadialAction::Picked(pick));
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Replace the choices. They stay until the template is reloaded, and
    /// anything deeper than three rings is dropped.
    pub fn set_items(&mut self, cx: &mut Cx, mut items: Vec<RadialNode>) {
        let mut dropped = Vec::new();
        prune(&mut items, 0, &mut dropped);
        for key in dropped {
            log!("RadialMenu: item \"{key}\" dropped: more than {MAX_DEPTH} rings deep");
        }
        self.items_from_rust = true;
        if self.tree != items {
            self.tree = items;
            self.reset_rings(cx);
        }
        self.redraw_menu(cx);
    }

    /// The key of the choice being aimed at, when it can be picked.
    pub fn hot_key(&self) -> Option<String> {
        let open = self.open.as_ref()?;
        let (depth, index) = open.hot?;
        self.node_at(&open.open_path, depth, index).map(|node| node.key.clone())
    }

    /// Where the rings are centred, in window coordinates.
    pub fn centre(&self) -> Option<DVec2> {
        self.open.as_ref().map(|open| open.centre)
    }

    /// The parents whose rings are open, by depth.
    pub fn open_path(&self) -> &[usize] {
        self.open.as_ref().map_or(&[], |open| &open.open_path)
    }

    /// Open the ring of the parent at `(depth, index)`.
    fn open_ring(&mut self, cx: &mut Cx, depth: usize, index: usize) {
        let Some(open) = self.open.as_ref() else { return };
        let Some(node) = self.node_at(&open.open_path, depth, index) else { return };
        if !Self::openable(node, depth) {
            return;
        }
        let id = node.id;
        let now = cx.seconds_since_app_start();
        if let Some(open) = self.open.as_mut() {
            open.open_path.truncate(depth);
            open.open_path.push(index);
            open.ring_opened_at[depth + 1] = now;
        }
        self.keep_growing(cx);
        self.redraw_menu(cx);
        let uid = self.uid;
        cx.widget_action(uid, RadialAction::RingOpened(id));
    }

    /// Bring the rings, the aim and the dwell up to date with a pointer at
    /// `at`.
    fn pointer_at(&mut self, cx: &mut Cx, at: DVec2) {
        let dwell_on = self.open_delay > 0.0;
        let mut changed = false;
        // Opening a ring can put the pointer over it, which may itself be a
        // crossing, so the pick runs again after every opening; the depth
        // limit bounds how often that can happen.
        for _ in 0..=MAX_DEPTH {
            let Some(open) = self.open.as_ref() else { return };
            let rings = self.rings_for(open);
            let d = at - open.centre;
            let hit = pick(&rings, self.bands().hub, d.x, d.y);
            let node = hit.and_then(|(depth, index)| self.node_at(&open.open_path, depth, index));
            let hot = match (hit, node) {
                (Some(hit), Some(node)) if node.enabled => Some(hit),
                _ => None,
            };
            let past_outer = hit.map_or(false, |(depth, _)| d.length() >= rings[depth].outer);
            let openable = match (hit, node) {
                (Some((depth, _)), Some(node)) => Self::openable(node, depth),
                _ => false,
            };
            let change = ring_change(&open.open_path, hit, past_outer, openable, dwell_on);
            cx.set_cursor(if hot.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
            if let Some(open) = self.open.as_mut() {
                changed |= open.hot != hot || open.aim != hit;
                open.hot = hot;
                open.aim = hit;
                open.pointer = Some(at);
            }
            match change {
                RingChange::Keep => self.stop_dwell(cx),
                RingChange::Truncate(n) => {
                    self.stop_dwell(cx);
                    if let Some(open) = self.open.as_mut() {
                        if open.open_path.len() > n {
                            open.open_path.truncate(n);
                            changed = true;
                        }
                    }
                }
                RingChange::Dwell { depth, index } => {
                    let waiting = self.open.as_ref().and_then(|open| open.dwell);
                    if let Some(open) = self.open.as_mut() {
                        if open.open_path.len() > depth + 1 {
                            open.open_path.truncate(depth + 1);
                            changed = true;
                        }
                    }
                    if waiting != Some((depth, index)) {
                        self.stop_dwell(cx);
                        self.dwell_timer = cx.start_timeout(self.open_delay);
                        if let Some(open) = self.open.as_mut() {
                            open.dwell = Some((depth, index));
                        }
                    }
                }
                RingChange::Open { depth, index } => {
                    self.stop_dwell(cx);
                    self.open_ring(cx, depth, index);
                    changed = true;
                    continue;
                }
            }
            break;
        }
        if changed {
            self.redraw_menu(cx);
        }
    }

    /// Aim at whatever is under `at` on the rings as they stand, opening and
    /// closing nothing. A press or a release is decided on what is on
    /// screen; only movement crosses a rim. A press just past ring 0's rim in
    /// a parent's direction is a press on that parent, and running the
    /// crossing rule for it would open the parent's ring and pick from a
    /// ring that was never shown.
    fn aim_at(&mut self, cx: &mut Cx, at: DVec2) {
        let Some(open) = self.open.as_ref() else { return };
        let rings = self.rings_for(open);
        let d = at - open.centre;
        let hit = pick(&rings, self.bands().hub, d.x, d.y);
        let node = hit.and_then(|(depth, index)| self.node_at(&open.open_path, depth, index));
        let hot = match (hit, node) {
            (Some(hit), Some(node)) if node.enabled => Some(hit),
            _ => None,
        };
        cx.set_cursor(if hot.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
        let Some(open) = self.open.as_mut() else { return };
        let changed = open.hot != hot || open.aim != hit;
        open.hot = hot;
        open.aim = hit;
        open.pointer = Some(at);
        if changed {
            self.redraw_menu(cx);
        }
    }

    /// The dwell ran out while the pointer was still on its target.
    fn dwell_fired(&mut self, cx: &mut Cx) {
        let Some(open) = self.open.as_mut() else { return };
        let Some((depth, index)) = open.dwell.take() else { return };
        if open.aim != Some((depth, index)) {
            return;
        }
        let path = open.open_path.clone();
        let pointer = open.pointer;
        let Some(node) = self.node_at(&path, depth, index) else { return };
        if Self::openable(node, depth) {
            if path.get(depth) != Some(&index) {
                self.open_ring(cx, depth, index);
                if let Some(at) = pointer {
                    self.pointer_at(cx, at);
                }
            }
        } else if path.len() > depth {
            if let Some(open) = self.open.as_mut() {
                open.open_path.truncate(depth);
            }
            self.redraw_menu(cx);
        }
    }

    fn release_at(&mut self, cx: &mut Cx, at: DVec2) {
        let Some(open) = self.open.as_mut() else { return };
        let opening = open.opening_press;
        open.opening_press = false;
        let counts = release_counts(opening, open.press_at, at, self.hub_radius);
        // The press that opened the menu is one stroke out from the centre,
        // and its release still crosses what it crossed, so a flick with no
        // movement reported on the way opens and picks. Any other release
        // ends a press made on the rings as shown, and acts on those.
        if opening {
            self.pointer_at(cx, at);
        } else {
            self.aim_at(cx, at);
        }
        let Some(open) = self.open.as_ref() else { return };
        let Some((depth, index)) = open.aim else {
            // Let go in the dead zone. The press that opened the menu, let
            // go where it opened, leaves it up for a second gesture; any
            // other release there means "none of these".
            if !opening {
                self.cancel(cx);
            }
            return;
        };
        if !counts {
            return;
        }
        let Some(node) = self.node_at(&open.open_path, depth, index) else { return };
        // A refusal is not a cancel: the menu stays up on a disabled choice.
        if !node.enabled {
            return;
        }
        if Self::openable(node, depth) {
            if open.open_path.get(depth) != Some(&index) {
                self.open_ring(cx, depth, index);
                self.pointer_at(cx, at);
            }
            return;
        }
        let mut path = open.open_path[..depth.min(open.open_path.len())].to_vec();
        path.push(index);
        let pick = RadialPick { id: node.id, key: node.key.clone(), path };
        self.choose(cx, pick);
    }

    fn press_while_open(&mut self, cx: &mut Cx, at: DVec2) {
        let Some(open) = self.open.as_ref() else { return };
        let r = (at - open.centre).length();
        let reach = self.bands().reach(self.rings_for(open).len() - 1);
        if press_is_outside(r, reach, self.outside_slop) {
            if self.take_down(cx, true) {
                self.swallow_up = true;
                let uid = self.uid;
                cx.widget_action(uid, RadialAction::Cancelled);
            }
        } else {
            self.aim_at(cx, at);
        }
    }

    fn key_down(&mut self, cx: &mut Cx, ke: &KeyEvent) {
        match ke.key_code {
            KeyCode::Escape => {
                // Nothing locked above the menu, or an overlay opened over it
                // loses its Escape to the menu whenever the menu hears the
                // key first.
                let on_top = cx.sweep_lock_area().map_or(true, |top| top == self.field_area);
                if on_top && claim_escape(cx) {
                    self.cancel(cx);
                }
            }
            KeyCode::Tab => {
                // While the menu is up its field is no tab stop (see
                // `draw_walk`), so the window finds no stop to move to and
                // scrolls nothing into view. Only a Tab in the frame the menu
                // opened in, before that draw, still finds the stop; the
                // window has moved the focus on by the time the tree sees
                // the key, and putting it back within the same event means
                // no focus change is ever reported, so Tab neither leaves
                // the menu nor takes it down.
                if !self.field_area.is_empty() {
                    cx.set_key_focus(self.field_area);
                }
            }
            KeyCode::Backspace => {
                let closed = self.open.as_mut().map_or(false, |open| open.open_path.pop().is_some());
                if closed {
                    self.stop_dwell(cx);
                    self.redraw_menu(cx);
                }
            }
            code => {
                let Some(open) = self.open.as_ref() else { return };
                let focus = self.rings_for(open).len() - 1;
                let path = open.open_path[..focus.min(open.open_path.len())].to_vec();
                let count = ring_nodes(&self.tree, &path, focus).map_or(0, |nodes| nodes.len());
                let Some((depth, index)) = digit_target(&path, code, count) else { return };
                let Some(node) = self.node_at(&path, depth, index) else { return };
                if !node.enabled {
                    return;
                }
                if Self::openable(node, depth) {
                    if let Some(open) = self.open.as_mut() {
                        open.open_path = path;
                    }
                    self.stop_dwell(cx);
                    self.open_ring(cx, depth, index);
                    return;
                }
                let mut chosen = path;
                chosen.push(index);
                let pick = RadialPick { id: node.id, key: node.key.clone(), path: chosen };
                self.choose(cx, pick);
            }
        }
    }

    /// Pointer and keyboard input while the menu is up. Answers whether the
    /// event was a press or release the menu took, so it is not also read as
    /// a press that opens.
    ///
    /// Raw events rather than hits: the rings reach far past the field, and
    /// `hits` only reports what lands on the field.
    fn handle_open_input(&mut self, cx: &mut Cx, event: &Event) -> bool {
        match event {
            Event::MouseMove(me) => {
                self.pointer_at(cx, me.abs);
                false
            }
            Event::MouseDown(me) => {
                if me.handled.get().is_empty() {
                    me.handled.set(self.field_area);
                }
                self.press_while_open(cx, me.abs);
                true
            }
            Event::MouseUp(me) => {
                self.release_at(cx, me.abs);
                true
            }
            Event::TouchUpdate(te) => {
                let followed = self.touch;
                let Some(touch) = te
                    .touches
                    .iter()
                    .find(|touch| followed.map_or(true, |uid| uid == touch.uid))
                else {
                    return false;
                };
                match touch.state {
                    TouchState::Start => {
                        self.touch = Some(touch.uid);
                        if touch.handled.get().is_empty() {
                            touch.handled.set(self.field_area);
                        }
                        self.press_while_open(cx, touch.abs);
                    }
                    TouchState::Move | TouchState::Stable => {
                        self.touch = Some(touch.uid);
                        self.pointer_at(cx, touch.abs);
                    }
                    TouchState::Stop => {
                        self.touch = None;
                        self.release_at(cx, touch.abs);
                    }
                }
                matches!(touch.state, TouchState::Start | TouchState::Stop)
            }
            Event::KeyDown(ke) => {
                self.key_down(cx, ke);
                false
            }
            // The menu holds the pointer, and a wheel is the pointer too. A
            // scroll view walks its children before it scrolls, and scrolls
            // by nothing marked handled, so the page under the rings stays
            // where the rings were opened over it instead of carrying the
            // panel away beneath them.
            Event::Scroll(se) => {
                se.handled_x.set(true);
                se.handled_y.set(true);
                false
            }
            _ => false,
        }
    }

    /// The keyboard's way in: Enter or Space on the field's tab stop opens
    /// the rings around the middle of the field, as a press there would, and
    /// the digits take it from there.
    fn opens_from_key(&self, cx: &Cx, ke: &KeyEvent) -> bool {
        let m = ke.modifiers;
        self.trigger != RadialTrigger::Manual
            && matches!(ke.key_code, KeyCode::ReturnKey | KeyCode::NumpadEnter | KeyCode::Space)
            && !(m.control || m.alt || m.logo)
            && !self.field_area.is_empty()
            && cx.has_key_focus(self.field_area)
    }

    fn draw_menu(&mut self, cx: &mut Cx2d, snapshot: Option<GaussBlurSnapshot>) {
        let Some(open) = self.open.clone() else { return };
        let rings = self.rings_for(&open);
        let bands = self.bands();
        let now = cx.seconds_since_app_start();
        let frosted = if self.look == RadialLook::Frosted { 1.0 } else { 0.0 };

        // Set before the first wedge, since the draw call takes its uniforms
        // and textures from here when it is made.
        self.draw_wedge
            .draw_vars
            .set_uniform(cx, live_id!(frost_level), &[self.frost_level as f32]);
        self.draw_wedge
            .draw_vars
            .set_uniform(cx, live_id!(frost_tint), &[self.frost_tint as f32]);
        bind_gauss_snapshot(&mut self.draw_wedge.draw_vars, cx, snapshot);

        let grows = self.grows(&open, rings.len(), now);
        for (depth, ring) in rings.iter().enumerate() {
            let grow = grows[depth].1;
            let nodes = ring_nodes(&self.tree, &open.open_path, depth).unwrap_or(&[]);
            // The drawn gap is cosmetic and the pick has none: a flick that
            // lands exactly in a seam still has a nearest wedge. Capped well
            // short of the step so a crowded ring keeps some wedge to draw.
            let half_gap = (self.gap.to_radians() * 0.5).min(ring.step() * 0.4);
            // The box follows the drawn reach, swing included, or a spring
            // would be cut off at the edge of its own box.
            let outer = drawn_outer(ring.inner, ring.outer, grow);
            for (i, node) in nodes.iter().enumerate().take(ring.count) {
                let (a0, a1) = ring.span_of(i);
                let a0 = (a0 + half_gap).rem_euclid(TAU);
                let a1 = (a1 - half_gap).rem_euclid(TAU);
                let rect = sector_bounds(open.centre, ring.inner, outer, a0, a1);
                let w = &mut self.draw_wedge;
                w.centre = vec2f((open.centre.x - rect.pos.x) as f32, (open.centre.y - rect.pos.y) as f32);
                w.a0 = a0 as f32;
                w.a1 = a1 as f32;
                w.inner = ring.inner as f32;
                w.outer = ring.outer as f32;
                w.grow = grow as f32;
                w.hot = if open.hot == Some((depth, i)) { 1.0 } else { 0.0 };
                w.open = if depth + 1 < rings.len() && open.open_path.get(depth) == Some(&i) { 1.0 } else { 0.0 };
                w.disabled = if node.enabled { 0.0 } else { 1.0 };
                w.has_children = if node.children.is_empty() { 0.0 } else { 1.0 };
                w.frosted = frosted;
                w.hub = 0.0;
                w.draw_abs(cx, rect);
            }
            if depth == 0 {
                let side = (bands.hub + WEDGE_PAD) * 2.0;
                let rect = Rect {
                    pos: dvec2(open.centre.x - side * 0.5, open.centre.y - side * 0.5),
                    size: dvec2(side, side),
                };
                let w = &mut self.draw_wedge;
                w.centre = vec2f((side * 0.5) as f32, (side * 0.5) as f32);
                w.inner = 0.0;
                w.outer = bands.hub.max(1.0) as f32;
                w.grow = grow as f32;
                w.hot = 0.0;
                w.open = 0.0;
                w.disabled = 0.0;
                w.has_children = 0.0;
                w.frosted = frosted;
                w.hub = 1.0;
                w.draw_abs(cx, rect);
            }
        }

        // Words last, so a crisp label is never under a wedge and glass
        // never samples one; and only on a ring whose enter time is up,
        // because a label over a wedge that has not reached it yet hangs in
        // empty space. The time and not the reach: a spring passes its edge
        // well before it has settled.
        let mut disabled_alpha = [0.38f32];
        self.draw_wedge
            .draw_vars
            .get_uniform(cx, live_id!(disabled_alpha), &mut disabled_alpha);
        let disabled_alpha = disabled_alpha[0].clamp(0.0, 1.0);
        let focus = rings.len() - 1;
        // Every word and glyph drawn, as boxes measured from the centre, for
        // the numbers to keep clear of. The focus ring is the outermost, so
        // by the time its numbers are placed every ring's words are in.
        let mut words: Vec<Rect> = Vec::new();
        for (depth, ring) in rings.iter().enumerate() {
            if grows[depth].0 < 1.0 {
                continue;
            }
            let half_gap = (self.gap.to_radians() * 0.5).min(ring.step() * 0.4);
            let nodes = ring_nodes(&self.tree, &open.open_path, depth).unwrap_or(&[]).to_vec();
            let mid = (ring.inner + ring.outer) * 0.5;
            for (i, node) in nodes.iter().enumerate().take(ring.count) {
                let (ox, oy) = ring.offset(i, mid);
                let at = dvec2(open.centre.x + ox, open.centre.y + oy);
                let hot = open.hot == Some((depth, i));
                let parent_open = depth + 1 < rings.len() && open.open_path.get(depth) == Some(&i);
                let draw = if hot {
                    &mut self.draw_label_hot
                } else if parent_open {
                    &mut self.draw_label_open
                } else {
                    &mut self.draw_label
                };
                let size = draw.text_style.font_size as f64;
                let color = draw.color;
                if !node.enabled {
                    draw.color.w = color.w * disabled_alpha;
                }
                // With a glyph the two stack on screen, the glyph over the
                // word, whichever way the wedge points: text is read
                // upright, not along the radius.
                let (label_dy, icon_dy) = if node.icon.is_empty() { (0.0, 0.0) } else { (0.45 * size, -0.55 * size) };
                let width = measure(draw, cx, &node.label);
                let pos = dvec2(at.x - width * 0.5, at.y + label_dy - size * 0.5 - size * INK_DROP);
                draw.draw_abs(cx, pos, &node.label);
                words.push(word_box(dvec2(ox, oy + label_dy), width, size));
                let ink = draw.color;
                draw.color = color;
                if !node.icon.is_empty() {
                    // The glyph takes its word's colour, so it stays legible
                    // on a hot or open wedge as the word does.
                    let icon_color = self.draw_icon.color;
                    self.draw_icon.color = ink;
                    let icon_size = self.draw_icon.text_style.font_size as f64;
                    let icon_width = measure(&self.draw_icon, cx, &node.icon);
                    let pos = dvec2(
                        at.x - icon_width * 0.5,
                        at.y + icon_dy - icon_size * 0.5 - icon_size * INK_DROP,
                    );
                    self.draw_icon.draw_abs(cx, pos, &node.icon);
                    self.draw_icon.color = icon_color;
                    words.push(word_box(dvec2(ox, oy + icon_dy), icon_width, icon_size));
                }
            }
            // Nine and no further, and only on the focus ring: there is no
            // tenth digit key, and the digits act on the outermost ring.
            if self.show_numbers && depth == focus {
                for i in 0..nodes.len().min(ring.count).min(9) {
                    let text = (i + 1).to_string();
                    let size = self.draw_number.text_style.font_size as f64;
                    let width = measure(&self.draw_number, cx, &text);
                    // Off every word: a word laid across a wedge that points
                    // sideways reaches in past where the number would sit.
                    let spot = number_spot(ring, i, half_gap, dvec2(width, size), &words);
                    let pos = dvec2(
                        open.centre.x + spot.x - width * 0.5,
                        open.centre.y + spot.y - size * 0.5 - size * INK_DROP,
                    );
                    // Over the hot wedge the digit takes the hot label's ink:
                    // the quiet meta colour is made for a resting face, and on
                    // the primary face it can all but vanish. The focus ring
                    // is the outermost, so no wedge on it is an open parent.
                    let resting = self.draw_number.color;
                    if open.hot == Some((depth, i)) {
                        self.draw_number.color = self.draw_label_hot.color;
                    }
                    self.draw_number.draw_abs(cx, pos, &text);
                    self.draw_number.color = resting;
                }
            }
        }

        // On the time as well: a spring is past 1 mid-swing, and stopping the
        // frames there would leave the ring frozen past its edge.
        if grows.iter().any(|(share, _)| *share < 1.0) {
            self.next_frame = cx.new_next_frame();
        }
    }
}

impl Drop for RadialMenu {
    fn drop(&mut self) {
        // Dropped holding the pointer (its page rebuilt under an open menu):
        // nothing else would ever let go of the lock, and every hit test in
        // the window would go on failing.
        if self.locked {
            orphan_sweep_locks(&[self.field_area]);
        }
    }
}

impl Widget for RadialMenu {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // Fit has nothing to measure — the rings are on the overlay — so a
        // Fit field is exactly nothing wide, for a menu opened from Rust.
        let walk = Walk {
            width: match walk.width {
                Size::Fit { .. } => Size::Fixed(0.0),
                other => other,
            },
            height: match walk.height {
                Size::Fit { .. } => Size::Fixed(0.0),
                other => other,
            },
            ..walk
        };
        self.draw_field.draw_walk(cx, walk);
        // Carried across redraws: the lock and the key focus were given the
        // old area, and a fresh one that nobody told them about would leave
        // the menu holding a grab on nothing. The place the keyboard goes
        // back to on close is carried too when it is the field, which it is
        // for a menu opened from its own tab stop.
        let drawn = self.draw_field.area();
        if !self.field_area.is_empty() && self.restore_focus == self.field_area {
            self.restore_focus = drawn;
        }
        self.field_area = cx.update_area_refs(self.field_area, drawn);
        // One stop for the whole menu, so Tab reaches it and Enter or Space
        // opens it without the pointer anywhere near. None for a menu only
        // Rust opens, which no key could open and a Fit field would make an
        // invisible stop; and none while the menu is up, so Tab finds no
        // stop past the menu's to move to and scrolls nothing into view
        // under the rings.
        if self.open.is_none() && self.trigger != RadialTrigger::Manual {
            cx.add_nav_stop(self.field_area, NavRole::TextInput, Inset::default());
        }
        self.pass_size = cx.current_pass_size();
        if self.open.is_some() {
            // Opened before the field was ever drawn: an Empty area can take
            // neither the lock nor the keyboard, so both were owed to the
            // first draw that gives it one.
            self.lock(cx);
            self.take_focus(cx);
        }

        if let Some(open) = self.open.as_ref() {
            if !open.fitted {
                if let Some(bounds) = self.bounds(cx) {
                    let full = self.fit_reach();
                    let centre = fit_centre(bounds, open.centre, full);
                    if let Some(open) = self.open.as_mut() {
                        open.centre = centre;
                        open.fitted = true;
                    }
                }
            }
        }

        // The overlay list is begun on every draw, open or not: a list that
        // is not begun keeps showing what it showed last.
        let Some(mut list) = self.draw_list.take() else {
            return DrawStep::done();
        };
        list.begin_overlay_reuse(cx);
        let snapshot = if self.open.is_some() && self.look == RadialLook::Frosted {
            request_window_gauss(cx)
        } else {
            None
        };
        cx.begin_root_turtle_for_pass(Layout::default());
        if self.open.is_some() {
            self.draw_menu(cx, snapshot);
        }
        cx.end_pass_sized_turtle();
        list.end(cx);
        self.draw_list = Some(list);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            self.redraw_menu(cx);
        }
        if self.dwell_timer.is_event(event).is_some() {
            self.dwell_timer = Timer::empty();
            self.dwell_fired(cx);
        }

        // The release that belongs to a dismissing press: eat it, then let
        // go of the pointer.
        if self.swallow_up {
            let released = match event {
                Event::MouseUp(_) => true,
                Event::TouchUpdate(te) => te.touches.iter().any(|touch| touch.state == TouchState::Stop),
                _ => false,
            };
            if released {
                self.swallow_up = false;
                if self.open.is_none() {
                    self.unlock(cx);
                }
                return;
            }
        }
        // A menu cannot outlive its window's focus, and a dismissing press
        // still waiting for its release will never get one.
        if matches!(event, Event::WindowLostFocus(_) | Event::Pause | Event::Background) {
            self.swallow_up = false;
            self.cancel(cx);
            self.unlock(cx);
            return;
        }
        if let Event::KeyFocus(kf) = event {
            if self.open.is_some() && kf.prev == self.field_area && kf.focus != self.field_area {
                self.cancel(cx);
            }
        }

        let mut taken = false;
        if self.open.is_none() {
            if let Event::KeyDown(ke) = event {
                if self.opens_from_key(cx, ke) {
                    let at = self.field_area.clipped_rect(cx).center();
                    self.touch = None;
                    self.open_with(cx, at, false);
                    return;
                }
            }
        }
        if self.open.is_some() {
            taken = self.handle_open_input(cx, event);
        } else if self.trigger == RadialTrigger::Secondary {
            // Raw events, so a primary press in the field reaches what is
            // underneath untouched: `hits` would capture it.
            // A press another overlay holds the pointer for is that overlay's
            // to answer. Raw events do not pass through the lock as hits do,
            // so the check is made here, or a right press outside an open
            // overlay would open this menu over the press that closes it.
            let free = cx.sweep_lock_area().map_or(true, |top| top == self.field_area);
            match event {
                Event::MouseDown(me) if free && me.button.is_secondary() && self.field_area.clipped_rect(cx).contains(me.abs) => {
                    me.handled.set(self.field_area);
                    self.touch = None;
                    self.open_with(cx, me.abs, true);
                    taken = true;
                }
                Event::LongPress(lp) if free && self.field_area.clipped_rect(cx).contains(lp.abs) => {
                    self.touch = Some(lp.uid);
                    self.open_with(cx, lp.abs, true);
                    taken = true;
                }
                _ => {}
            }
        }
        if self.trigger == RadialTrigger::Press && !taken && self.open.is_none() {
            if let Hit::FingerDown(fe) = event.hits(cx, self.field_area) {
                if fe.is_primary_hit() {
                    self.touch = None;
                    self.open_with(cx, fe.abs, true);
                }
            }
        }
    }

    /// The label of the choice being aimed at, so a test can read the menu
    /// in one line.
    fn text(&self) -> String {
        let Some(open) = self.open.as_ref() else { return String::new() };
        open.hot
            .and_then(|(depth, index)| self.node_at(&open.open_path, depth, index))
            .map(|node| node.label.clone())
            .unwrap_or_default()
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        let Some(open) = self.open.as_ref() else {
            return Some(format_snapshot(None));
        };
        let rings = self.rings_for(open);
        let parents: Vec<String> = (1..rings.len())
            .filter_map(|depth| self.node_at(&open.open_path, depth - 1, open.open_path[depth - 1]))
            .map(|node| node.key.clone())
            .collect();
        let hot = self.hot_key();
        Some(format_snapshot(Some((open.centre, rings.len() - 1, hot.as_deref(), &parents))))
    }

    /// Every choice on the open rings, so a test can find "Mail" by its id
    /// or its word and press the middle of its rect. The rect is a square on
    /// the word's point, half the ring deep and never wider than half the
    /// wedge's arc there, so its middle is always on the wedge the pick
    /// reads, whatever frame of a grow the tree is read in.
    fn snapshot_parts(&self, _cx: &Cx) -> Vec<SnapshotPart> {
        let Some(open) = self.open.as_ref() else { return Vec::new() };
        let rings = self.rings_for(open);
        let mut parts = Vec::new();
        for (depth, ring) in rings.iter().enumerate() {
            let nodes = ring_nodes(&self.tree, &open.open_path, depth).unwrap_or(&[]);
            let mid = (ring.inner + ring.outer) * 0.5;
            let side = ((ring.outer - ring.inner) * 0.5).min(ring.step() * mid * 0.5).max(1.0);
            for (i, node) in nodes.iter().enumerate().take(ring.count) {
                let (ox, oy) = ring.offset(i, mid);
                parts.push(SnapshotPart {
                    id: node.id,
                    widget_type: "RadialMenuItem",
                    rect: Rect {
                        pos: dvec2(open.centre.x + ox - side * 0.5, open.centre.y + oy - side * 0.5),
                        size: dvec2(side, side),
                    },
                    text: node.label.clone(),
                    selected: open.hot == Some((depth, i)),
                    enabled: node.enabled,
                });
            }
        }
        parts
    }
}

fn find_node(nodes: &[RadialNode], id: LiveId) -> Option<&RadialNode> {
    nodes
        .iter()
        .find_map(|node| if node.id == id { Some(node) } else { find_node(&node.children, id) })
}

impl RadialMenuRef {
    pub fn open_at(&self, cx: &mut Cx, at: DVec2) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open_at(cx, at);
        }
    }

    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.close(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.borrow().map(|inner| inner.is_open()).unwrap_or(false)
    }

    pub fn set_items(&self, cx: &mut Cx, items: Vec<RadialNode>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_items(cx, items);
        }
    }

    /// Every report this menu made in `actions`. One event can carry more
    /// than one — a ring opening and then a pick — so the first alone is not
    /// enough.
    fn reports(&self, actions: &Actions) -> Vec<RadialAction> {
        let uid = self.widget_uid();
        actions
            .iter()
            .filter_map(|action| action.as_widget_action())
            .filter(|action| action.widget_uid == uid)
            .map(|action| action.cast::<RadialAction>())
            .collect()
    }

    /// The choice picked this pass, if one was.
    pub fn picked(&self, actions: &Actions) -> Option<RadialPick> {
        self.reports(actions).into_iter().find_map(|report| match report {
            RadialAction::Picked(pick) => Some(pick),
            _ => None,
        })
    }

    /// Whether the menu was taken down this pass without a choice.
    pub fn cancelled(&self, actions: &Actions) -> bool {
        self.reports(actions).contains(&RadialAction::Cancelled)
    }

    /// The parent whose outer ring opened this pass, if one did.
    pub fn ring_opened(&self, actions: &Actions) -> Option<LiveId> {
        self.reports(actions).into_iter().find_map(|report| match report {
            RadialAction::RingOpened(id) => Some(id),
            _ => None,
        })
    }

    /// The label of the choice with this id, for a caller that wants the
    /// word back rather than the key.
    pub fn label_of(&self, id: LiveId) -> String {
        self.borrow()
            .and_then(|inner| find_node(&inner.tree, id).map(|node| node.label.clone()))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::makepad_platform::event::{ScrollEvent, ScrollPhase};
    use crate::makepad_script::trap::NoTrap;
    use crate::pie_menu::PieRing;

    const DEG: f64 = PI / 180.0;

    /// A pointer `points` from the centre, `deg` clockwise from straight up,
    /// in screen coordinates where y grows downward.
    fn aim(deg: f64, points: f64) -> (f64, f64) {
        let a = deg * DEG;
        (a.sin() * points, -a.cos() * points)
    }

    fn keys(nodes: &[RadialNode]) -> Vec<String> {
        nodes
            .iter()
            .map(|node| {
                if node.children.is_empty() {
                    node.key.clone()
                } else {
                    format!("{}{{{}}}", node.key, keys(&node.children).join(", "))
                }
            })
            .collect()
    }

    fn defaults() -> RingBands {
        RingBands::new(30.0, 96.0, 64.0, 4.0)
    }

    /// The flat list is read top to bottom: a child needs its parent above
    /// it, the first of two equal keys wins, and nothing deeper than three
    /// rings or with an empty segment survives. Every drop is reported, in
    /// the order the list declared it, so the log names each mistake.
    #[test]
    fn build_tree_keeps_order_and_drops_orphans_duplicates_and_depth() {
        let flat: Vec<(&str, &str, &str, bool)> = ["a", "b", "b/x", "b/y", "c/z", "b/x/q", "b/x/q/deep", "a", "b//w"]
            .iter()
            .map(|key| (*key, "label", "", true))
            .collect();
        let (tree, dropped) = build_tree(&flat);
        assert_eq!(keys(&tree), vec!["a".to_string(), "b{b/x{b/x/q}, b/y}".to_string()]);
        assert_eq!(dropped, vec!["c/z", "b/x/q/deep", "a", "b//w"]);
        assert_eq!(tree[1].children[0].id, LiveId::from_str("b/x"), "the id is the full path's");
        let (empty, dropped) = build_tree(&[("", "nothing", "", true)]);
        assert!(empty.is_empty() && dropped == vec![String::new()], "an empty key is an empty segment");
    }

    /// Nodes built in Rust carry their full path too, however deep they
    /// were built before being placed.
    #[test]
    fn children_carry_their_parents_key() {
        let share = RadialNode::new("share", "Share").children(vec![
            RadialNode::new("mail", "Mail"),
            RadialNode::new("export", "Export").children(vec![RadialNode::new("text", "Text")]),
        ]);
        assert_eq!(share.children[0].key, "share/mail");
        assert_eq!(share.children[1].children[0].key, "share/export/text");
        assert_eq!(share.children[1].children[0].id, LiveId::from_str("share/export/text"));
        let mut deep = vec![RadialNode::new("a", "A").children(vec![RadialNode::new("b", "B")
            .children(vec![RadialNode::new("c", "C").children(vec![RadialNode::new("d", "D")])])])];
        let mut dropped = Vec::new();
        prune(&mut deep, 0, &mut dropped);
        assert_eq!(dropped, vec!["a/b/c/d"]);
        assert_eq!(levels(&deep), MAX_DEPTH);
    }

    /// A full ring is the pie menu's ring: the same wedge for every
    /// direction. The directions sit a quarter degree off the half-degree
    /// grid, clear of every seam, since which side a direction exactly on a
    /// seam falls to is promised by neither.
    #[test]
    fn a_full_arc_ring_agrees_with_pie_ring() {
        for count in 1..=8 {
            let arc = ArcRing::full(count, 20.0, 96.0);
            let pie = PieRing::new(count, 20.0);
            for step in 0..720 {
                let (dx, dy) = aim((step as f64 + 0.25) * 0.5, 100.0);
                let a = dx.atan2(-dy).rem_euclid(TAU);
                assert_eq!(arc.angle_index(a), pie.pick(dx, dy), "{count} wedges at step {step}");
            }
            for i in 0..count {
                assert!((arc.centre(i) - pie.centre(i)).abs() < 1e-9, "centre {i} of {count}");
            }
        }
    }

    /// The middle of a child arc is its parent's direction, however much the
    /// arc had to widen — including the parent at twelve o'clock, whose
    /// children straddle the seam.
    #[test]
    fn a_child_arc_is_centred_on_its_parent() {
        let roots = ArcRing::full(6, 30.0, 96.0);
        let (inner, outer) = (100.0, 164.0);
        let three = ArcRing::child(&roots, 1, 3, inner, outer, 44.0);
        assert!((three.span - 60.0 * DEG).abs() < 1e-9, "three children keep the parent's 60 degrees");
        for parent in 0..6 {
            let four = ArcRing::child(&roots, parent, 4, inner, outer, 44.0);
            assert!((four.span / DEG - 76.39).abs() < 0.01, "four children widen to {}", four.span / DEG);
            let middle = (four.start + four.span * 0.5).rem_euclid(TAU);
            let off = (middle - roots.centre(parent) + PI).rem_euclid(TAU) - PI;
            assert!(off.abs() < 1e-9, "parent {parent}: the arc's middle is {off} off its parent");
        }
        let straddling = ArcRing::child(&roots, 0, 4, inner, outer, 44.0);
        assert!((straddling.start / DEG - 321.80).abs() < 0.01, "start {}", straddling.start / DEG);
        assert!((straddling.centre(1) / DEG - 350.45).abs() < 0.01, "child 1 at {}", straddling.centre(1) / DEG);
        assert!((straddling.centre(2) / DEG - 9.55).abs() < 0.01, "child 2 at {}", straddling.centre(2) / DEG);
        let crowded = ArcRing::child(&roots, 2, 40, inner, outer, 44.0);
        assert_eq!(crowded.span, TAU, "forty children take the whole circle and no more");
    }

    fn pick_at(rings: &[ArcRing], deg: f64, r: f64) -> Option<(usize, usize)> {
        let (dx, dy) = aim(deg, r);
        pick(rings, 30.0, dx, dy)
    }

    /// Bands are read outward from the hub. An outer ring owns its gap and
    /// everything beyond, within its arc; outside that arc the direction
    /// falls back to ring 0.
    #[test]
    fn the_pick_reads_bands_outward_and_falls_back_outside_an_arc() {
        let roots = ArcRing::full(6, 30.0, 96.0);
        let rings = [roots, ArcRing::child(&roots, 1, 3, 100.0, 164.0, 44.0)];
        assert_eq!(pick_at(&rings, 60.0, 29.0), None, "the hub");
        assert_eq!(pick_at(&rings, 60.0, 60.0), Some((0, 1)), "ring 0");
        assert_eq!(pick_at(&rings, 60.0, 98.0), Some((1, 1)), "the gap belongs outward");
        assert_eq!(pick_at(&rings, 60.0, 4000.0), Some((1, 1)), "and so does a mile out");
        assert_eq!(pick_at(&rings, 100.0, 130.0), Some((0, 2)), "outside ring 1's arc");
        assert_eq!(pick_at(&rings, 35.0, 130.0), Some((1, 0)), "the first child");
    }

    /// With three rings open the deepest arc that holds the direction wins;
    /// a direction outside it but inside the ring beneath picks that ring.
    #[test]
    fn the_deepest_ring_wins_over_the_one_beneath() {
        let bands = defaults();
        let roots = ArcRing::full(6, bands.band(0).0, bands.band(0).1);
        let ring1 = ArcRing::child(&roots, 1, 3, bands.band(1).0, bands.band(1).1, 44.0);
        let ring2 = ArcRing::child(&ring1, 2, 3, bands.band(2).0, bands.band(2).1, 44.0);
        let rings = [roots, ring1, ring2];
        // Ring 2 is 37.8 degrees about the last child's 80.
        assert_eq!(pick_at(&rings, 80.0, 300.0), Some((2, 1)));
        assert_eq!(pick_at(&rings, 95.0, 300.0), Some((2, 2)));
        assert_eq!(pick_at(&rings, 40.0, 300.0), Some((1, 0)), "outside ring 2's arc");
        assert_eq!(pick_at(&rings, 80.0, 150.0), Some((1, 2)), "short of ring 2's band");
    }

    /// The defaults give the radii the widget's box and the fit assume.
    #[test]
    fn ring_radii_climb_and_reach_matches_the_box() {
        let bands = defaults();
        assert_eq!(bands.hub, 30.0);
        assert_eq!(bands.band(0), (30.0, 96.0));
        assert_eq!(bands.band(1), (100.0, 164.0));
        assert_eq!(bands.band(2), (168.0, 232.0));
        assert_eq!(bands.reach(2), 232.0);
        assert_eq!(bands.reach(2) * 2.0 + 8.0, 472.0, "the full-depth disc, pad included");
        let wide_hub = RingBands::new(120.0, 96.0, 64.0, 4.0);
        assert_eq!(wide_hub.band(0), (120.0, 128.0), "a hub past the radius still leaves a ring");
    }

    /// A wedge's box holds the bulge where it crosses up, right, down or
    /// left, not just its four corners.
    #[test]
    fn sector_bounds_includes_the_extremes_it_crosses() {
        let rect = sector_bounds(dvec2(0.0, 0.0), 100.0, 164.0, 330.0 * DEG, 30.0 * DEG);
        assert!((rect.pos.x - (-84.0)).abs() < 0.01, "{rect:?}");
        assert!((rect.pos.y - (-166.0)).abs() < 0.01, "{rect:?}");
        assert!((rect.size.x - 168.0).abs() < 0.01, "{rect:?}");
        assert!((rect.size.y - 81.40).abs() < 0.01, "{rect:?}");
        let right = sector_bounds(dvec2(0.0, 0.0), 100.0, 164.0, 60.0 * DEG, 120.0 * DEG);
        assert!(right.pos.x + right.size.x >= 164.0 && right.pos.y <= 0.0 && right.pos.y + right.size.y >= 0.0, "{right:?}");
        let whole = sector_bounds(dvec2(10.0, 10.0), 30.0, 96.0, PI, PI);
        assert!((whole.size.x - 196.0).abs() < 1e-9 && (whole.size.y - 196.0).abs() < 1e-9, "{whole:?}");
    }

    /// Inward is back at once; outward waits unless the hand crosses.
    #[test]
    fn ring_rule_crossing_opens_at_once_and_inward_is_back() {
        use RingChange::*;
        assert_eq!(ring_change(&[1, 0], None, false, false, true), Truncate(0), "the hub closes everything");
        assert_eq!(ring_change(&[], Some((0, 1)), true, true, true), Open { depth: 0, index: 1 }, "crossing");
        assert_eq!(ring_change(&[], Some((0, 1)), false, true, true), Dwell { depth: 0, index: 1 }, "resting");
        assert_eq!(ring_change(&[1], Some((0, 1)), false, true, true), Keep, "its ring is already open");
        assert_eq!(ring_change(&[1], Some((0, 1)), true, true, true), Keep, "crossing into its own open ring");
        assert_eq!(ring_change(&[1, 0], Some((0, 1)), false, true, true), Truncate(1), "band 0 closes ring 2");
        assert_eq!(ring_change(&[1], Some((0, 3)), false, false, true), Dwell { depth: 0, index: 3 }, "a leaf beside a sibling's ring");
        assert_eq!(ring_change(&[], Some((0, 1)), false, true, false), Keep, "resting does nothing with no delay");
        assert_eq!(ring_change(&[], Some((0, 1)), true, true, false), Open { depth: 0, index: 1 }, "crossing still opens");
        assert_eq!(ring_change(&[], Some((0, 3)), false, false, true), Keep, "a leaf with nothing open");
    }

    /// The digits count from one on the deepest open ring.
    #[test]
    fn digits_act_on_the_focus_ring() {
        assert_eq!(digit_target(&[1], KeyCode::Key2, 4), Some((1, 1)));
        assert_eq!(digit_target(&[1], KeyCode::Numpad4, 4), Some((1, 3)));
        assert_eq!(digit_target(&[1], KeyCode::Key0, 4), None, "no zero");
        assert_eq!(digit_target(&[1], KeyCode::Key5, 4), None, "past the count");
        assert_eq!(digit_target(&[], KeyCode::Key6, 6), Some((0, 5)));
    }

    /// The opening press has to travel a hub's width before its release
    /// picks; any other release counts wherever it lands.
    #[test]
    fn the_opening_release_needs_travel() {
        let press = dvec2(100.0, 100.0);
        assert!(!release_counts(true, press, dvec2(129.0, 100.0), 30.0));
        assert!(release_counts(true, press, dvec2(130.0, 100.0), 30.0));
        assert!(release_counts(false, press, press, 30.0));
    }

    #[test]
    fn a_press_is_outside_only_past_the_slop() {
        assert!(!press_is_outside(120.0, 96.0, 24.0));
        assert!(press_is_outside(120.01, 96.0, 24.0));
    }

    /// The centre is fitted against the full depth, inside the safe area
    /// and the edge margin, with the pie menu's own arithmetic.
    #[test]
    fn the_centre_is_fitted_for_the_deepest_ring() {
        let bounds = fit_bounds(dvec2(800.0, 600.0), 0.0, 0.0, 0.0, 0.0);
        assert_eq!(bounds, Rect { pos: dvec2(6.0, 6.0), size: dvec2(788.0, 588.0) });
        assert_eq!(fit_centre(bounds, dvec2(20.0, 20.0), 232.0), dvec2(242.0, 242.0));
        let notched = fit_bounds(dvec2(800.0, 600.0), 10.0, 40.0, 0.0, 0.0);
        assert_eq!(notched.pos, dvec2(16.0, 46.0));
    }

    #[test]
    fn snapshot_value_spells_the_state() {
        assert_eq!(format_snapshot(None), "closed");
        let rings = vec!["share".to_string()];
        assert_eq!(
            format_snapshot(Some((dvec2(399.6, 300.2), 1, Some("share/mail"), &rings))),
            "open at=400,300 focus=1 hot=share/mail rings=share"
        );
        let none: Vec<String> = Vec::new();
        assert_eq!(format_snapshot(Some((dvec2(10.0, 20.0), 0, None, &none))), "open at=10,20 focus=0 hot=- rings=");
    }

    /// A reduced-motion reader gets every ring at full size at once, on any
    /// curve, and a zero time is the same cut.
    #[test]
    fn reduced_motion_lands_every_grow_at_once() {
        for ease in [Ease::Linear, Ease::OutElastic, Ease::InElastic] {
            assert_eq!(grow_at(0.0, 0.12, true, &ease), 1.0, "{ease:?}");
            assert_eq!(grow_share(0.0, 0.12, true), 1.0);
            assert_eq!(grow_at(0.0, 0.0, false, &ease), 1.0, "{ease:?}");
        }
        assert_eq!(grow_share(0.06, 0.12, false), 0.5);
        assert_eq!(drawn_outer(30.0, 96.0, -0.2), 30.0, "a dip stops at the hole");
    }

    /// The theme's easings, in the order Foundations > Motion plays them.
    const EASE_TOKENS: [&str; 8] = [
        "motion_ease_standard",
        "motion_ease_standard_decelerate",
        "motion_ease_standard_accelerate",
        "motion_ease_emphasized_decelerate",
        "motion_ease_emphasized_accelerate",
        "motion_ease_linear",
        "motion_ease_spring",
        "motion_ease_bounce",
    ];

    /// The easing a theme token names, read out of the loaded theme as the
    /// Foundations page reads it, so these checks follow the theme and never
    /// restate it.
    fn theme_ease(vm: &mut ScriptVm, token: &str) -> Ease {
        let theme = vm.module(id!(theme));
        let value = vm.bx.heap.value(theme, LiveId::from_str(token).into(), NoTrap);
        Ease::script_from_value(vm, value)
    }

    fn theme_number(vm: &mut ScriptVm, token: &str) -> f64 {
        let theme = vm.module(id!(theme));
        vm.bx
            .heap
            .value(theme, LiveId::from_str(token).into(), NoTrap)
            .as_f64()
            .unwrap_or_else(|| panic!("the theme has no number {token}"))
    }

    /// Each of the theme's eight easings drives the reach exactly as the
    /// theme's own `Ease` evaluates it at the share of the time gone, and
    /// once the time is up the ring rests on its edge whatever the curve's
    /// last value. The shapes are checked too, so a token that came to name
    /// another curve would show: the decelerating default is most of the
    /// way out by a quarter of the time, the accelerating one short of half
    /// at half, the spring past the edge somewhere on the way, the bounce
    /// never.
    #[test]
    fn the_rings_grow_on_the_theme_easings() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let secs = theme_number(vm, "motion_short_3");
            assert!(secs > 0.0);
            for token in EASE_TOKENS {
                let ease = theme_ease(vm, token);
                for share in [0.0, 0.125, 0.25, 0.5, 0.75, 0.875] {
                    let elapsed = share * secs;
                    let grow = grow_at(elapsed, secs, false, &ease);
                    let theme = ease.map(elapsed / secs);
                    assert!((grow - theme).abs() < 1e-12, "{token} at {share}: {grow} against the theme's {theme}");
                }
                assert_eq!(grow_at(secs, secs, false, &ease), 1.0, "{token} rests on its edge");
                assert_eq!(grow_at(secs * 3.0, secs, false, &ease), 1.0, "{token} stays there");
                assert_eq!(grow_at(0.0, secs, true, &ease), 1.0, "{token} with reduced motion");
            }
            let widest = |ease: &Ease| {
                (1..100)
                    .map(|i| grow_at(i as f64 * 0.01 * secs, secs, false, ease))
                    .fold(f64::MIN, f64::max)
            };
            let decelerate = theme_ease(vm, "motion_ease_emphasized_decelerate");
            assert!(grow_at(0.25 * secs, secs, false, &decelerate) > 0.75);
            let accelerate = theme_ease(vm, "motion_ease_standard_accelerate");
            assert!(grow_at(0.5 * secs, secs, false, &accelerate) < 0.5);
            assert!(widest(&theme_ease(vm, "motion_ease_spring")) > 1.0, "a spring swings past its edge");
            assert!(widest(&theme_ease(vm, "motion_ease_bounce")) <= 1.0, "a bounce never does");
        });
    }

    /// The menu's own motion is the theme's tokens, read from the theme and
    /// not restated, and a page can hand it any other easing token.
    #[test]
    fn the_enter_motion_defaults_to_the_theme_tokens() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let value = crate::script_eval!(vm, {use mod.widgets.* RadialMenu{}});
            let menu = RadialMenu::script_from_value(vm, value);
            assert_eq!(menu.enter_ease, theme_ease(vm, "motion_ease_emphasized_decelerate"));
            assert_eq!(menu.enter_secs, theme_number(vm, "motion_short_3"));
            assert_eq!(menu.ring_enter_secs, theme_number(vm, "motion_short_2"));
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                RadialMenu{enter_ease: theme.motion_ease_spring}
            });
            let sprung = RadialMenu::script_from_value(vm, value);
            assert_eq!(sprung.enter_ease, theme_ease(vm, "motion_ease_spring"));
            assert_ne!(sprung.enter_ease, menu.enter_ease);
        });
    }

    /// On the spring, ring 0 is drawn well past its edge for part of its
    /// enter, and the menu still answers by the edge it comes to rest at: a
    /// pointer just past that edge is a crossing that opens the parent's
    /// ring, and a press there with no slop is off the menu, though the
    /// swinging ring is drawn under both.
    #[test]
    fn an_overshooting_ring_is_picked_by_its_resting_edge() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                RadialMenu{enter_ease: theme.motion_ease_spring outside_slop: 0.0}
            });
            let mut menu = RadialMenu::script_from_value(vm, value);
            menu.pass_size = dvec2(800.0, 600.0);
            vm.with_cx_mut(|cx| {
                menu.set_items(cx, page_tree());
                menu.open_at(cx, dvec2(400.0, 300.0));
                let open = menu.open.clone().expect("open");
                let c = open.centre;
                let (inner, outer) = menu.bands().band(0);
                // The widest swing, found on the curve the menu is on.
                let secs = menu.enter_secs;
                let grow = (1..100)
                    .map(|i| menu.grows(&open, 1, open.opened_at + i as f64 * 0.01 * secs)[0].1)
                    .fold(f64::MIN, f64::max);
                let reach = drawn_outer(inner, outer, grow);
                assert!(reach > outer + 10.0, "the spring swings past the edge: {grow}, drawn to {reach}");
                assert_eq!(menu.rings_for(&open)[0].outer, outer, "the ring the pick reads keeps its resting edge");
                let past = outer + 6.0;
                menu.pointer_at(cx, toward(c, 60.0, past));
                assert_eq!(menu.open_path(), &[1], "just past the resting edge is a crossing, swing or not");
                menu.close(cx);
                menu.open_at(cx, dvec2(400.0, 300.0));
                menu.press_while_open(cx, toward(c, 180.0, past));
                assert!(!menu.is_open(), "a press past the resting edge is off the menu");
            });
        });
    }

    /// The enums are written qualified in the DSL: their bare names would
    /// shadow words other widgets already use.
    #[test]
    fn no_enum_is_splatted() {
        let source = include_str!("radial_menu.rs");
        for name in ["RadialLook", "RadialTrigger"] {
            let needle = format!("splat(mod.widgets.{name})");
            assert!(!source.contains(&needle), "{name} is splatted");
        }
    }

    /// The wedge shader is compiled nowhere else, and an error in it would
    /// only show as rings that never paint.
    #[test]
    fn the_wedge_shader_compiles() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let value = crate::script_eval!(vm, {
                mod.shader.test_compile_draw_source(mod.widgets.RadialMenu.draw_wedge, "glsl", false)
            });
            let text = vm
                .bx
                .heap
                .string_with(value, |_heap, text| text.to_string())
                .expect("the compiler answers with source");
            assert!(!text.starts_with("ERRORS:"), "the wedge did not compile: {text}");
            // The frosted branch and the pyramid it samples made it in: a shader
            // whose branch was dropped would still compile and never frost.
            for name in ["io_frost", "io_sample_blur", "tex_mip5_texture", "uni_frost_tint", "uni_color_wedge_open_rim"] {
                assert!(text.contains(name), "the compiled wedge has no {name}");
            }
        });
    }

    /// The words of every open ring laid out as the draw lays them, each a
    /// box on its wedge's middle radius. The widths are wider than the
    /// face's own, so a spot that clears these clears the drawn words.
    fn laid_out_words(tree: &[RadialNode], path: &[usize], rings: &[ArcRing]) -> Vec<(String, Rect)> {
        let mut words = Vec::new();
        for (depth, ring) in rings.iter().enumerate() {
            let mid = (ring.inner + ring.outer) * 0.5;
            for (i, node) in ring_nodes(tree, path, depth).unwrap().iter().enumerate() {
                let (x, y) = ring.offset(i, mid);
                let width = node.label.chars().count() as f64 * 7.0 + 2.0;
                words.push((node.label.clone(), word_box(dvec2(x, y), width, 10.0)));
            }
        }
        words
    }

    /// On every ring the catalogue opens, in the first panel, the context
    /// menu and the frosted one, each number keeps clear of every word and
    /// sits on its own wedge: a pick at any corner of its box is the choice
    /// it numbers. Placed where it used to be, on the wedge's middle line at
    /// a fixed depth, the number runs into the words the review named: the
    /// E of Export, the P of Picture, Document, Paste and Delete.
    #[test]
    fn a_number_keeps_clear_of_every_word() {
        let bands = defaults();
        let half_gap = 1.0 * DEG;
        let digit = dvec2(8.0, 8.0);
        let (context, _) = build_tree(&[
            ("open", "Open", "", true),
            ("arrange", "Arrange", "", true),
            ("arrange/front", "To front", "", true),
            ("arrange/back", "To back", "", true),
            ("remove", "Remove", "", true),
        ]);
        let (frosted, _) = build_tree(&[
            ("move", "Move", "", true),
            ("share", "Share", "", true),
            ("share/mail", "Mail", "", true),
            ("share/link", "Link", "", true),
            ("share/print", "Print", "", true),
            ("rotate", "Rotate", "", true),
            ("scale", "Scale", "", true),
            ("delete", "Delete", "", true),
        ]);
        let page = page_tree();
        let cases: [(&[RadialNode], &[usize]); 8] = [
            (&page, &[]),
            (&page, &[1]),
            (&page, &[1, 3]),
            (&page, &[4]),
            (&context, &[]),
            (&context, &[1]),
            (&frosted, &[]),
            (&frosted, &[1]),
        ];
        let mut overprinted = Vec::new();
        for (tree, path) in cases {
            let rings = open_rings(tree, path, bands, 44.0);
            let words = laid_out_words(tree, path, &rings);
            let boxes: Vec<Rect> = words.iter().map(|(_, word)| *word).collect();
            let focus = rings.len() - 1;
            let ring = &rings[focus];
            for i in 0..ring.count.min(9) {
                let spot = number_spot(ring, i, half_gap, digit, &boxes);
                let number = word_box(spot, digit.x, digit.y);
                for (word, at) in &words {
                    assert!(!boxes_touch(&number, at, NUMBER_CLEAR), "{path:?}: number {} on {word}", i + 1);
                }
                let (x0, y0) = (number.pos.x, number.pos.y);
                let (x1, y1) = (x0 + number.size.x, y0 + number.size.y);
                for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1), (spot.x, spot.y)] {
                    assert_eq!(pick(&rings, bands.hub, x, y), Some((focus, i)), "{path:?}: number {} leaves its wedge", i + 1);
                }
                let (ox, oy) = ring.offset(i, ring.inner + (ring.outer - ring.inner) * NUMBER_AT);
                let old = word_box(dvec2(ox, oy), digit.x, digit.y);
                for (word, at) in &words {
                    if boxes_touch(&old, at, 0.0) {
                        overprinted.push(format!("{} {word}", i + 1));
                    }
                }
            }
        }
        for named in ["4 Export", "1 Picture", "2 Document", "3 Paste", "5 Delete"] {
            assert!(overprinted.iter().any(|hit| hit == named), "the fixed spot no longer overprints {named}: {overprinted:?}");
        }
    }

    /// A number with nothing in its way stays on its wedge's middle line at
    /// the depth it always had, so only a crowded wedge's number moves.
    #[test]
    fn a_number_with_room_stays_where_it_was() {
        let roots = ArcRing::full(6, 30.0, 96.0);
        let spot = number_spot(&roots, 0, 1.0 * DEG, dvec2(8.0, 8.0), &[]);
        let (x, y) = roots.offset(0, 30.0 + 66.0 * NUMBER_AT);
        assert!((spot.x - x).abs() < 1e-9 && (spot.y - y).abs() < 1e-9, "{spot:?}");
    }

    /// The farthest a curve swings, found on the curve: 1 for one that only
    /// settles, past 1 for the spring.
    #[test]
    fn the_swing_is_read_off_the_curve() {
        assert_eq!(ease_peak(&Ease::Linear), 1.0);
        let finest = (0..=100_000).map(|i| Ease::OutElastic.map(i as f64 / 100_000.0)).fold(f64::MIN, f64::max);
        assert!((ease_peak(&Ease::OutElastic) - finest).abs() < 1e-4, "{} against {finest}", ease_peak(&Ease::OutElastic));
        assert!(finest > 1.37, "the spring's swing, a little past where its sine peaks: {finest}");
        let bands = defaults();
        assert_eq!(fit_reach(bands, 3, 1.0, 1.0), 232.0, "at rest, the deepest ring's edge");
        assert!((fit_reach(bands, 3, 1.0, 1.354) - (168.0 + 64.0 * 1.354)).abs() < 1e-9);
        assert!((fit_reach(bands, 1, 1.354, 1.354) - (30.0 + 66.0 * 1.354)).abs() < 1e-9, "one ring");
    }

    /// Opened in a corner on the spring, the centre is moved in far enough
    /// for the widest swing of the deepest ring, so no frame of the swing is
    /// cut off by the window; without the swing to make room for, only the
    /// resting reach is kept.
    #[test]
    fn the_centre_is_fitted_for_the_swing() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                RadialMenu{enter_ease: theme.motion_ease_spring}
            });
            let mut menu = RadialMenu::script_from_value(vm, value);
            menu.pass_size = dvec2(800.0, 600.0);
            vm.with_cx_mut(|cx| {
                menu.set_items(cx, page_tree());
                let bands = menu.bands();
                let widest = (0..MAX_DEPTH)
                    .map(|depth| {
                        let secs = if depth == 0 { menu.enter_secs } else { menu.ring_enter_secs };
                        let (inner, outer) = bands.band(depth);
                        (1..400)
                            .map(|k| drawn_outer(inner, outer, grow_at(k as f64 / 400.0 * secs, secs, false, &menu.enter_ease)))
                            .fold(0.0, f64::max)
                    })
                    .fold(0.0, f64::max);
                assert!(widest > 232.0 + 10.0, "the spring swings the third ring past its edge: {widest}");
                for corner in [dvec2(20.0, 20.0), dvec2(780.0, 580.0)] {
                    menu.open_at(cx, corner);
                    let c = menu.centre().unwrap();
                    assert!(c.x - widest >= EDGE - 1e-6 && c.y - widest >= EDGE - 1e-6, "{corner:?}: {c:?} swings {widest} out");
                    assert!(c.x + widest <= 800.0 - EDGE + 1e-6 && c.y + widest <= 600.0 - EDGE + 1e-6, "{corner:?}: {c:?} swings {widest} out");
                    menu.close(cx);
                }
                menu.reduced_motion = true;
                menu.open_at(cx, dvec2(20.0, 20.0));
                assert_eq!(menu.centre(), Some(dvec2(242.0, 242.0)), "no swing, only the resting reach");
            });
        });
    }

    fn rgb_of(color: Vec4f) -> [f64; 3] {
        [color.x as f64, color.y as f64, color.z as f64]
    }

    fn luminance(rgb: [f64; 3]) -> f64 {
        let lin = |c: f64| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
    }

    fn contrast(a: [f64; 3], b: [f64; 3]) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    fn theme_color(vm: &mut ScriptVm, theme: &str, token: &str) -> [f64; 3] {
        let themes = vm.module(id!(themes));
        let table = vm
            .bx
            .heap
            .value(themes, LiveId::from_str(theme).into(), NoTrap)
            .as_object()
            .unwrap_or_else(|| panic!("no theme {theme}"));
        let value = vm.bx.heap.value(table, LiveId::from_str(token).into(), NoTrap);
        assert!(value.as_color().is_some() || value.as_pod().is_some(), "{theme} has no colour {token}");
        rgb_of(Vec4f::script_from_value(vm, value))
    }

    /// In the skeleton theme the open parent's face and a resting face are
    /// all but the same grey, so the face alone marks nothing there. The
    /// band along the open parent's rim does, in every theme: it stands off
    /// the face it is drawn on by 3:1 and off a resting wedge by 2:1, and it
    /// is in the compiled wedge.
    #[test]
    fn the_open_parent_is_marked_in_every_theme() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            for theme in ["dark", "light", "skeleton"] {
                let resting = theme_color(vm, theme, "color_surface_container_high");
                let open = theme_color(vm, theme, "color_secondary_container");
                let rim = theme_color(vm, theme, "color_secondary");
                assert!(contrast(rim, open) >= 3.0, "{theme}: the band on the open face is {:.2}:1", contrast(rim, open));
                assert!(contrast(rim, resting) >= 2.0, "{theme}: the band against a resting face is {:.2}:1", contrast(rim, resting));
            }
            let (open, resting) = (
                theme_color(vm, "skeleton", "color_secondary_container"),
                theme_color(vm, "skeleton", "color_surface_container_high"),
            );
            assert!(contrast(open, resting) < 1.1, "the skeleton's faces alone: {:.3}:1", contrast(open, resting));
            let value = crate::script_eval!(vm, {
                mod.shader.test_compile_draw_source(mod.widgets.RadialMenu.draw_wedge, "glsl", false)
            });
            let text = vm.bx.heap.string_with(value, |_heap, text| text.to_string()).expect("source");
            assert!(text.contains("uni_color_wedge_open_rim"), "the wedge draws no band");
        });
    }

    /// The catalogue page's tree, built from Rust.
    fn page_tree() -> Vec<RadialNode> {
        vec![
            RadialNode::new("move", "Move"),
            RadialNode::new("share", "Share").children(vec![
                RadialNode::new("mail", "Mail"),
                RadialNode::new("link", "Link"),
                RadialNode::new("print", "Print"),
                RadialNode::new("export", "Export").children(vec![
                    RadialNode::new("picture", "Picture"),
                    RadialNode::new("document", "Document"),
                    RadialNode::new("text", "Text"),
                ]),
            ]),
            RadialNode::new("rotate", "Rotate"),
            RadialNode::new("scale", "Scale"),
            RadialNode::new("edit", "Edit").children(vec![
                RadialNode::new("cut", "Cut"),
                RadialNode::new("copy", "Copy"),
                RadialNode::new("paste", "Paste").enabled(false),
            ]),
            RadialNode::new("delete", "Delete"),
        ]
    }

    /// A point `points` from `centre`, `deg` clockwise from straight up.
    fn toward(centre: DVec2, deg: f64, points: f64) -> DVec2 {
        let (dx, dy) = aim(deg, points);
        dvec2(centre.x + dx, centre.y + dy)
    }

    fn reports_in(actions: &[Action]) -> Vec<RadialAction> {
        actions
            .iter()
            .filter_map(|action| action.as_widget_action())
            .map(|action| action.cast::<RadialAction>())
            .collect()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent { key_code: code, ..Default::default() }
    }

    /// Run `drive` against a menu holding the page's tree in an 800 by 600
    /// window, answering what it reported. The widget's own pointer and key
    /// paths run; only the platform's event plumbing is left out.
    fn drive(drive: impl FnOnce(&mut RadialMenu, &mut Cx)) -> Vec<RadialAction> {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        // The rings time their growth and their dwell against the app clock.
        cx.init_cx_os();
        let mut reports = Vec::new();
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let value = crate::script_eval!(vm, {use mod.widgets.* RadialMenu{}});
            let mut menu = RadialMenu::script_from_value(vm, value);
            menu.pass_size = dvec2(800.0, 600.0);
            vm.with_cx_mut(|cx| {
                menu.set_items(cx, page_tree());
                let actions = cx.capture_actions(|cx| drive(&mut menu, cx));
                reports = reports_in(&actions);
            });
        });
        reports
    }

    fn picked(key: &str, path: &[usize]) -> RadialAction {
        RadialAction::Picked(RadialPick { id: LiveId::from_str(key), key: key.to_string(), path: path.to_vec() })
    }

    /// Press, then out through Share's rim and on to Mail: the crossing
    /// opens Share's ring at once, the ring's middle is Share's direction,
    /// and the release picks two rings deep.
    #[test]
    fn one_stroke_picks_two_rings_deep() {
        let reports = drive(|menu, cx| {
            menu.open_with(cx, dvec2(400.0, 300.0), true);
            let c = menu.centre().unwrap();
            assert_eq!(c, dvec2(400.0, 300.0), "room to spare: left where it was asked for");
            menu.pointer_at(cx, toward(c, 60.0, 60.0));
            assert_eq!(menu.hot_key().as_deref(), Some("share"));
            assert!(menu.open_path().is_empty(), "resting has not opened it yet");
            menu.pointer_at(cx, toward(c, 50.0, 110.0));
            assert_eq!(menu.open_path(), &[1], "crossing Share's rim opened its ring");
            menu.pointer_at(cx, toward(c, 30.0, 130.0));
            assert_eq!(
                menu.snapshot_value(cx),
                Some("open at=400,300 focus=1 hot=share/mail rings=share".to_string())
            );
            assert_eq!(menu.text(), "Mail");
            menu.release_at(cx, toward(c, 30.0, 130.0));
            assert!(!menu.is_open());
        });
        assert_eq!(reports.first(), Some(&RadialAction::Opened));
        assert!(reports.contains(&RadialAction::RingOpened(LiveId::from_str("share"))));
        assert_eq!(reports.last(), Some(&picked("share/mail", &[1, 0])));
    }

    /// Two crossings in one stroke reach the third ring.
    #[test]
    fn one_stroke_reaches_the_third_ring() {
        let reports = drive(|menu, cx| {
            menu.open_with(cx, dvec2(400.0, 300.0), true);
            let c = menu.centre().unwrap();
            menu.pointer_at(cx, toward(c, 60.0, 60.0));
            menu.pointer_at(cx, toward(c, 88.0, 110.0));
            assert_eq!(menu.hot_key().as_deref(), Some("share/export"));
            menu.pointer_at(cx, toward(c, 88.0, 180.0));
            assert_eq!(menu.open_path(), &[1, 3], "past Export's rim its ring opened");
            menu.pointer_at(cx, toward(c, 76.0, 200.0));
            assert_eq!(menu.hot_key().as_deref(), Some("share/export/picture"));
            menu.release_at(cx, toward(c, 76.0, 200.0));
        });
        assert_eq!(reports.last(), Some(&picked("share/export/picture", &[1, 3, 0])));
    }

    /// Resting on a parent opens its ring when the wait ends. A disabled
    /// choice refuses a release without taking the menu down, and moving
    /// back into the hub closes every outer ring.
    #[test]
    fn resting_opens_a_ring_and_inward_is_back() {
        let reports = drive(|menu, cx| {
            menu.open_with(cx, dvec2(400.0, 300.0), true);
            let c = menu.centre().unwrap();
            menu.pointer_at(cx, toward(c, 240.0, 60.0));
            assert_eq!(menu.hot_key().as_deref(), Some("edit"));
            assert!(menu.open_path().is_empty());
            menu.dwell_fired(cx);
            assert_eq!(menu.open_path(), &[4], "the wait ran out on Edit");
            menu.pointer_at(cx, toward(c, 260.0, 130.0));
            assert_eq!(menu.hot_key(), None, "Paste is aimed at but cannot be picked");
            menu.release_at(cx, toward(c, 260.0, 130.0));
            assert!(menu.is_open(), "a refusal is not a cancel");
            menu.pointer_at(cx, toward(c, 0.0, 10.0));
            assert!(menu.open_path().is_empty(), "the hub closes every outer ring");
            assert_eq!(menu.snapshot_value(cx), Some("open at=400,300 focus=0 hot=- rings=".to_string()));
            menu.release_at(cx, toward(c, 0.0, 10.0));
            assert!(!menu.is_open(), "a later release in the hub is none of these");
        });
        assert_eq!(reports.last(), Some(&RadialAction::Cancelled));
        assert!(!reports.iter().any(|report| matches!(report, RadialAction::Picked(_))));
    }

    /// The press that opened the menu, let go where it opened, leaves it up;
    /// and near an edge a release short of a hub's travel picks nothing even
    /// though the nudged centre put a wedge under it.
    #[test]
    fn the_opening_press_neither_cancels_nor_picks_by_accident() {
        let reports = drive(|menu, cx| {
            menu.open_with(cx, dvec2(400.0, 300.0), true);
            menu.release_at(cx, dvec2(400.0, 300.0));
            assert!(menu.is_open(), "left up for a second gesture");
            menu.close(cx);
            menu.open_with(cx, dvec2(20.0, 20.0), true);
            assert_eq!(menu.centre(), Some(dvec2(242.0, 242.0)), "fitted for the deepest ring");
            menu.pointer_at(cx, dvec2(24.0, 24.0));
            assert!(menu.hot_key().is_some(), "the nudge put a wedge under the press");
            menu.release_at(cx, dvec2(24.0, 24.0));
            assert!(menu.is_open(), "too little travel to pick");
        });
        assert!(!reports
            .iter()
            .any(|report| matches!(report, RadialAction::Picked(_) | RadialAction::Cancelled)));
    }

    /// A press well past the outermost ring takes the menu down and leaves
    /// its release still to be eaten; one inside the slop is on the menu.
    /// The lock held until that release is driven through the event path in
    /// `the_dismissing_release_is_eaten_before_the_lock_goes`.
    #[test]
    fn a_press_past_the_slop_cancels_and_eats_its_release() {
        let reports = drive(|menu, cx| {
            menu.open_at(cx, dvec2(400.0, 300.0));
            menu.press_while_open(cx, dvec2(400.0, 300.0 - 119.0));
            assert!(menu.is_open(), "inside the slop");
            menu.press_while_open(cx, dvec2(400.0, 300.0 - 200.0));
            assert!(!menu.is_open());
            assert!(menu.swallow_up, "the release is still to be eaten");
        });
        assert_eq!(reports.last(), Some(&RadialAction::Cancelled));
    }

    /// Digits act on the outermost ring and follow it outward, Backspace
    /// steps back one ring, and Escape takes the whole menu down.
    #[test]
    fn the_keyboard_walks_the_rings() {
        let reports = drive(|menu, cx| {
            menu.open_at(cx, dvec2(400.0, 300.0));
            menu.key_down(cx, &key(KeyCode::Key2));
            assert_eq!(menu.open_path(), &[1], "2 opened Share");
            menu.key_down(cx, &key(KeyCode::Key4));
            assert_eq!(menu.open_path(), &[1, 3], "4 on Share's ring opened Export");
            menu.key_down(cx, &key(KeyCode::Key9));
            assert_eq!(menu.open_path(), &[1, 3], "past the count does nothing");
            menu.key_down(cx, &key(KeyCode::Backspace));
            assert_eq!(menu.open_path(), &[1]);
            menu.key_down(cx, &key(KeyCode::Escape));
            assert!(!menu.is_open());
            menu.open_at(cx, dvec2(400.0, 300.0));
            menu.key_down(cx, &key(KeyCode::Key5));
            menu.key_down(cx, &key(KeyCode::Key3));
            assert!(menu.is_open(), "Paste is disabled");
            menu.key_down(cx, &key(KeyCode::Key1));
        });
        assert!(reports.contains(&RadialAction::Cancelled));
        assert_eq!(reports.last(), Some(&picked("edit/cut", &[4, 0])));
    }

    /// A menu declared in the DSL builds its tree from the flat list, and the
    /// presets change only what they say they change.
    #[test]
    fn the_dsl_list_becomes_the_tree() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let value = crate::script_eval!(vm, {
                use mod.widgets.*
                RadialMenuContext{
                    items: [
                        RadialItem{key: "open" label: "Open"}
                        RadialItem{key: "arrange" label: "Arrange"}
                        RadialItem{key: "arrange/front" label: "To front"}
                        RadialItem{key: "arrange/back" label: "To back" enabled: false}
                        RadialItem{key: "stray/child" label: "Nowhere"}
                    ]
                }
            });
            let menu = RadialMenu::script_from_value(vm, value);
            assert_eq!(menu.trigger, RadialTrigger::Secondary);
            assert_eq!(menu.look, RadialLook::Solid);
            assert_eq!(keys(&menu.tree), vec!["open".to_string(), "arrange{arrange/front, arrange/back}".to_string()]);
            assert!(!menu.tree[1].children[1].enabled);
            assert!(!menu.is_open());
            assert_eq!(menu.snapshot_value(vm.cx()), Some("closed".to_string()));
            let frosted = crate::script_eval!(vm, {use mod.widgets.* RadialMenuFrosted{}});
            let frosted = RadialMenu::script_from_value(vm, frosted);
            assert_eq!(frosted.look, RadialLook::Frosted);
            assert_eq!(frosted.trigger, RadialTrigger::Press);
        });
    }

    /// Clicked open, then a press just past ring 0's rim in Share's
    /// direction: a press on Share. Its release opens Share's ring and
    /// picks nothing from a ring that was not on screen when it was made.
    #[test]
    fn a_press_past_a_rim_acts_on_the_rings_shown() {
        let reports = drive(|menu, cx| {
            menu.open_at(cx, dvec2(400.0, 300.0));
            let c = menu.centre().unwrap();
            let at = toward(c, 55.0, 110.0);
            menu.press_while_open(cx, at);
            assert!(menu.open_path().is_empty(), "a press opens no ring");
            assert_eq!(menu.hot_key().as_deref(), Some("share"));
            menu.release_at(cx, at);
            assert!(menu.is_open(), "a release on a parent keeps the menu up");
            assert_eq!(menu.open_path(), &[1], "and opens the parent's ring");
            assert_eq!(menu.hot_key().as_deref(), Some("share/link"), "the ring now shown is what the pointer is over");
        });
        assert!(!reports.iter().any(|report| matches!(report, RadialAction::Picked(_))), "{reports:?}");
        assert!(reports.contains(&RadialAction::RingOpened(LiveId::from_str("share"))));
    }

    /// One frame of `root` into a window-less pass of `size`, with the
    /// overlay a window gives the tree to hang its own lists off.
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

        fn draw(&mut self, cx: &mut Cx, root: &WidgetRef, size: DVec2) {
            self.pass.set_size(cx, size);
            let event = DrawEvent::default();
            let mut draw = crate::makepad_draw::cx_draw::CxDraw::new(cx, &event);
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

    fn drawn_cx() -> Cx {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        cx
    }

    /// A menu opened only from Rust above a context menu's field, both
    /// holding the page's tree, drawn once into an 800 by 600 window.
    fn two_menus(cx: &mut Cx) -> (WidgetRef, Target) {
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: 800
                    height: 600
                    flow: Down
                    subject := RadialMenu{width: 200 height: 200 trigger: RadialTrigger.Manual}
                    context := RadialMenuContext{width: 200 height: 200}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        for path in [ids!(subject), ids!(context)] {
            root.widget(cx, path).as_radial_menu().set_items(cx, page_tree());
        }
        let mut target = Target::new(cx);
        target.draw(cx, &root, dvec2(800.0, 600.0));
        (root, target)
    }

    fn field_of(root: &WidgetRef, cx: &Cx, path: &[LiveId]) -> Area {
        root.widget(cx, path).borrow::<RadialMenu>().expect("a radial menu").field_area
    }

    fn mouse_down(abs: DVec2, button: MouseButton) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            handled: std::cell::Cell::new(Area::Empty),
            time: 0.0,
        })
    }

    fn mouse_up(abs: DVec2, button: MouseButton) -> Event {
        Event::MouseUp(MouseUpEvent {
            abs,
            button,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            time: 0.0,
        })
    }

    /// Moves the key focus the way the event loop does between events.
    fn settle_focus(cx: &mut Cx) {
        cx.action(());
        cx.handle_actions();
    }

    /// A right press on the context field while another menu holds the
    /// pointer is that menu's press: it closes that menu and opens nothing.
    /// Once the pointer is free the same press opens the context menu.
    #[test]
    fn a_right_press_another_overlay_holds_opens_nothing() {
        let mut cx = drawn_cx();
        let (root, _target) = two_menus(&mut cx);
        let subject = root.widget(&cx, ids!(subject)).as_radial_menu();
        let context = root.widget(&cx, ids!(context)).as_radial_menu();
        subject.open_at(&mut cx, dvec2(400.0, 300.0));
        let held = field_of(&root, &cx, ids!(subject));
        assert_eq!(cx.sweep_lock_area(), Some(held), "the open menu holds the pointer");

        // On the context field, and far outside the open rings.
        let at = dvec2(100.0, 300.0);
        root.handle_event(&mut cx, &mouse_down(at, MouseButton::SECONDARY), &mut Scope::empty());
        assert!(!context.is_open(), "the press belonged to the menu holding the pointer");
        assert!(!subject.is_open(), "and it closed that menu");
        root.handle_event(&mut cx, &mouse_up(at, MouseButton::SECONDARY), &mut Scope::empty());
        assert_eq!(cx.sweep_lock_area(), None, "the dismissing release let go");

        root.handle_event(&mut cx, &mouse_down(at, MouseButton::SECONDARY), &mut Scope::empty());
        assert!(context.is_open(), "with the pointer free the same press opens it");
    }

    /// Opened before its first draw the field has no area, which can take
    /// neither the lock nor the keyboard; the first draw takes both.
    #[test]
    fn opened_before_its_first_draw_it_takes_the_pointer_when_drawn() {
        let mut cx = drawn_cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.widgets.*
                RadialMenu{width: 200 height: 200 trigger: RadialTrigger.Manual}
            });
            WidgetRef::script_from_value(vm, value)
        });
        root.as_radial_menu().set_items(&mut cx, page_tree());
        root.as_radial_menu().open_at(&mut cx, dvec2(400.0, 300.0));
        assert_eq!(cx.sweep_lock_area(), None, "nothing drawn to lock with");
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root, dvec2(800.0, 600.0));
        let field = root.borrow::<RadialMenu>().unwrap().field_area;
        assert!(!field.is_empty());
        assert_eq!(cx.sweep_lock_area(), Some(field), "the first draw took the lock");
        settle_focus(&mut cx);
        assert!(cx.has_key_focus(field), "and gave the field the keyboard");
    }

    /// The menu takes the keyboard for its digits and gives it back to
    /// where it was when it closes.
    #[test]
    fn closing_gives_the_keyboard_back() {
        let mut cx = drawn_cx();
        let (root, _target) = two_menus(&mut cx);
        let before = field_of(&root, &cx, ids!(context));
        cx.set_key_focus(before);
        settle_focus(&mut cx);
        assert!(cx.has_key_focus(before));
        let subject = root.widget(&cx, ids!(subject)).as_radial_menu();
        subject.open_at(&mut cx, dvec2(400.0, 300.0));
        settle_focus(&mut cx);
        assert!(cx.has_key_focus(field_of(&root, &cx, ids!(subject))), "the menu took the keyboard");
        root.handle_event(&mut cx, &Event::KeyDown(key(KeyCode::Escape)), &mut Scope::empty());
        assert!(!subject.is_open());
        settle_focus(&mut cx);
        assert!(cx.has_key_focus(before), "closing gave it back");
    }

    /// An overlay locked above the open menu has the Escape, whichever of
    /// the two hears it first.
    #[test]
    fn escape_belongs_to_an_overlay_locked_above() {
        let mut cx = drawn_cx();
        let (root, _target) = two_menus(&mut cx);
        let subject = root.widget(&cx, ids!(subject)).as_radial_menu();
        subject.open_at(&mut cx, dvec2(400.0, 300.0));
        let above = field_of(&root, &cx, ids!(context));
        cx.sweep_lock(above);
        root.handle_event(&mut cx, &Event::KeyDown(key(KeyCode::Escape)), &mut Scope::empty());
        assert!(subject.is_open(), "the overlay above had the key");
        cx.sweep_unlock(above);
        root.handle_event(&mut cx, &Event::KeyDown(key(KeyCode::Escape)), &mut Scope::empty());
        assert!(!subject.is_open());
    }

    /// Through the event path: a press past the rings closes the menu but
    /// holds the pointer, and its release is eaten before the lock goes.
    #[test]
    fn the_dismissing_release_is_eaten_before_the_lock_goes() {
        let mut cx = drawn_cx();
        let (root, _target) = two_menus(&mut cx);
        let subject = root.widget(&cx, ids!(subject)).as_radial_menu();
        subject.open_at(&mut cx, dvec2(400.0, 300.0));
        let held = field_of(&root, &cx, ids!(subject));
        let far = dvec2(790.0, 590.0);
        root.handle_event(&mut cx, &mouse_down(far, MouseButton::PRIMARY), &mut Scope::empty());
        assert!(!subject.is_open());
        assert_eq!(cx.sweep_lock_area(), Some(held), "held until the release");
        root.handle_event(&mut cx, &mouse_up(far, MouseButton::PRIMARY), &mut Scope::empty());
        assert_eq!(cx.sweep_lock_area(), None);
    }

    /// Dropped while open, the menu cannot let go of the pointer itself,
    /// having no `Cx`; it leaves the lock for the next event to release.
    #[test]
    fn a_menu_dropped_open_leaves_its_lock_for_the_next_event() {
        let mut cx = drawn_cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.widgets.*
                RadialMenu{width: 200 height: 200 trigger: RadialTrigger.Manual}
            });
            WidgetRef::script_from_value(vm, value)
        });
        root.as_radial_menu().set_items(&mut cx, page_tree());
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root, dvec2(800.0, 600.0));
        root.as_radial_menu().open_at(&mut cx, dvec2(400.0, 300.0));
        assert!(cx.sweep_lock_area().is_some());
        drop(root);
        assert!(cx.sweep_lock_area().is_some(), "nothing could let go of it on drop");
        crate::overlay_place::release_orphaned_sweep_locks(&mut cx);
        assert_eq!(cx.sweep_lock_area(), None, "the next event does");
    }

    /// Every tab stop in the frame `target` drew last, in tab order.
    fn nav_stops(cx: &mut Cx, target: &Target) -> Vec<Area> {
        let mut stops = Vec::new();
        crate::makepad_draw::cx_draw::CxDraw::iterate_nav_stops(cx, target.draw_list.draw_list_id(), |_, stop| {
            stops.push(stop.area);
            None
        });
        stops
    }

    /// The window's Tab, from the field of an open menu, finds no stop to
    /// move to, so it neither moves the keyboard nor scrolls the page to
    /// bring a next stop into view under the rings. Closed, the same Tab
    /// moves on to the next menu's stop.
    #[test]
    fn tab_under_an_open_menu_finds_nowhere_to_go() {
        let mut cx = drawn_cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: 800
                    height: 600
                    flow: Down
                    first := RadialMenu{width: 200 height: 200}
                    second := RadialMenuContext{width: 200 height: 200}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        for path in [ids!(first), ids!(second)] {
            root.widget(&cx, path).as_radial_menu().set_items(&mut cx, page_tree());
        }
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root, dvec2(800.0, 600.0));
        let field = field_of(&root, &cx, ids!(first));
        let mut nav = cx.with_vm(|vm| crate::nav_control::NavControl::script_new(vm));
        let list = target.draw_list.draw_list_id();
        let tab = Event::KeyDown(key(KeyCode::Tab));

        cx.set_key_focus(field);
        settle_focus(&mut cx);
        nav.handle_event(&mut cx, &tab, list);
        settle_focus(&mut cx);
        assert!(!cx.has_key_focus(field), "closed, Tab moves on from the menu's stop");

        cx.set_key_focus(field);
        settle_focus(&mut cx);
        let first = root.widget(&cx, ids!(first)).as_radial_menu();
        first.open_at(&mut cx, dvec2(400.0, 300.0));
        target.draw(&mut cx, &root, dvec2(800.0, 600.0));
        settle_focus(&mut cx);
        assert!(!nav_stops(&mut cx, &target).contains(&field_of(&root, &cx, ids!(first))), "an open menu is no stop");
        nav.handle_event(&mut cx, &tab, list);
        settle_focus(&mut cx);
        assert!(cx.has_key_focus(field_of(&root, &cx, ids!(first))), "open, Tab found no stop to move to");
        assert!(first.is_open());
    }

    /// A wheel over the page while the menu is up scrolls nothing: the page
    /// under the rings stays where it was when they opened. Closed, the same
    /// wheel scrolls the page.
    #[test]
    fn the_wheel_under_an_open_menu_scrolls_nothing() {
        let mut cx = drawn_cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                ScrollYView{
                    width: 800
                    height: 300
                    flow: Down
                    subject := RadialMenu{width: Fill height: 200 trigger: RadialTrigger.Manual}
                    View{width: Fill height: 1200}
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        root.widget(&cx, ids!(subject)).as_radial_menu().set_items(&mut cx, page_tree());
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root, dvec2(800.0, 300.0));
        let wheel = || {
            Event::Scroll(ScrollEvent {
                window_id: WindowId(1, 1),
                scroll: dvec2(0.0, 40.0),
                abs: dvec2(400.0, 150.0),
                modifiers: KeyModifiers::default(),
                handled_x: std::cell::Cell::new(false),
                handled_y: std::cell::Cell::new(false),
                is_mouse: false,
                time: 0.0,
                phase: ScrollPhase::None,
            })
        };
        let scrolled = |root: &WidgetRef| {
            root.borrow::<crate::view::View>().and_then(|view| view.scroll_extent()).map(|extent| extent.pos.y)
        };
        let subject = root.widget(&cx, ids!(subject)).as_radial_menu();

        subject.open_at(&mut cx, dvec2(400.0, 150.0));
        let before = scrolled(&root);
        let event = wheel();
        root.handle_event(&mut cx, &event, &mut Scope::empty());
        assert!(subject.is_open());
        assert_eq!(scrolled(&root), before, "the page did not move under the rings");
        let Event::Scroll(se) = &event else { unreachable!() };
        assert!(se.handled_y.get() && se.handled_x.get(), "the menu took the wheel");

        subject.close(&mut cx);
        root.handle_event(&mut cx, &wheel(), &mut Scope::empty());
        assert_ne!(scrolled(&root), before, "closed, the same wheel scrolls the page");
    }

    /// Enter or Space on a menu's tab stop opens the rings around the middle
    /// of its field, fitted as a press there would be, and while they are up
    /// the field is no stop. A menu only Rust opens is never a stop, and no
    /// key opens it.
    #[test]
    fn a_key_on_the_tab_stop_opens_the_rings() {
        let mut cx = drawn_cx();
        let (root, mut target) = two_menus(&mut cx);
        let subject = root.widget(&cx, ids!(subject)).as_radial_menu();
        let context = root.widget(&cx, ids!(context)).as_radial_menu();
        let manual = field_of(&root, &cx, ids!(subject));
        let field = field_of(&root, &cx, ids!(context));
        let stops = nav_stops(&mut cx, &target);
        assert!(stops.contains(&field), "the context menu is a stop");
        assert!(!stops.contains(&manual), "a menu only Rust opens is not");

        cx.set_key_focus(manual);
        settle_focus(&mut cx);
        root.handle_event(&mut cx, &Event::KeyDown(key(KeyCode::ReturnKey)), &mut Scope::empty());
        assert!(!subject.is_open(), "no key opens a menu only Rust opens");

        cx.set_key_focus(field);
        settle_focus(&mut cx);
        root.handle_event(&mut cx, &Event::KeyDown(key(KeyCode::Space)), &mut Scope::empty());
        assert!(context.is_open(), "Space on the stop opened it");
        let middle = field.clipped_rect(&cx).center();
        let want = fit_centre(fit_bounds(dvec2(800.0, 600.0), 0.0, 0.0, 0.0, 0.0), middle, 232.0);
        assert_eq!(root.widget(&cx, ids!(context)).borrow::<RadialMenu>().unwrap().centre(), Some(want));
        target.draw(&mut cx, &root, dvec2(800.0, 600.0));
        assert!(!nav_stops(&mut cx, &target).contains(&field_of(&root, &cx, ids!(context))), "no stop while it is up");
        root.handle_event(&mut cx, &Event::KeyDown(key(KeyCode::Escape)), &mut Scope::empty());
        assert!(!context.is_open());
        root.handle_event(&mut cx, &Event::KeyDown(key(KeyCode::ReturnKey)), &mut Scope::empty());
        settle_focus(&mut cx);
        assert!(context.is_open(), "Enter opens it too, the keyboard given back to the stop");
    }

    /// Opened from its tab stop and closed again, by Escape or by a pick,
    /// after the page under the rings was drawn again, the menu gives the
    /// keyboard back to the stop as it is now: Enter opens the rings again
    /// and Tab moves on from there. The handle the stop had when the menu
    /// opened names nothing by then.
    #[test]
    fn the_keyboard_is_back_on_the_stop_after_the_page_is_drawn_again() {
        let mut cx = drawn_cx();
        let (root, mut target) = two_menus(&mut cx);
        let context = root.widget(&cx, ids!(context)).as_radial_menu();
        cx.set_key_focus(field_of(&root, &cx, ids!(context)));
        settle_focus(&mut cx);
        for close in [KeyCode::Escape, KeyCode::Key1] {
            let opened_on = field_of(&root, &cx, ids!(context));
            root.handle_event(&mut cx, &Event::KeyDown(key(KeyCode::ReturnKey)), &mut Scope::empty());
            settle_focus(&mut cx);
            assert!(context.is_open(), "Enter on the stop opened it before {close:?}");
            root.redraw(&mut cx);
            target.draw(&mut cx, &root, dvec2(800.0, 600.0));
            let field = field_of(&root, &cx, ids!(context));
            assert_ne!(field, opened_on, "the page was drawn again under the rings");
            root.handle_event(&mut cx, &Event::KeyDown(key(close)), &mut Scope::empty());
            settle_focus(&mut cx);
            assert!(!context.is_open(), "{close:?} closed it");
            assert!(cx.has_key_focus(field), "the keyboard is back on the stop after {close:?}");
            target.draw(&mut cx, &root, dvec2(800.0, 600.0));
        }
        root.handle_event(&mut cx, &Event::KeyDown(key(KeyCode::ReturnKey)), &mut Scope::empty());
        assert!(context.is_open(), "and Enter opens it once more");
    }

    /// With the rings up, every choice on them is a part of the tree, its
    /// rect's middle on the wedge the pick reads, the aimed-at one selected
    /// and the one that cannot be picked marked so. Closed, there are none.
    #[test]
    fn the_open_rings_are_parts_of_the_tree() {
        drive(|menu, cx| {
            assert!(menu.snapshot_parts(cx).is_empty(), "closed");
            menu.open_at(cx, dvec2(400.0, 300.0));
            menu.key_down(cx, &key(KeyCode::Key5));
            let c = menu.centre().unwrap();
            menu.pointer_at(cx, toward(c, 240.0, 60.0));
            let parts = menu.snapshot_parts(cx);
            assert_eq!(parts.len(), 6 + 3, "ring 0 and Edit's ring");
            let open = menu.open.clone().unwrap();
            let rings = menu.rings_for(&open);
            for part in &parts {
                let at = part.rect.center() - c;
                let (depth, index) = pick(&rings, menu.bands().hub, at.x, at.y).expect("on a wedge");
                let node = menu.node_at(&open.open_path, depth, index).unwrap();
                assert_eq!((part.id, part.text.as_str()), (node.id, node.label.as_str()));
                assert_eq!(part.widget_type, "RadialMenuItem");
                assert_eq!(part.enabled, node.enabled);
            }
            let selected: Vec<&str> = parts.iter().filter(|part| part.selected).map(|part| part.text.as_str()).collect();
            assert_eq!(selected, vec!["Edit"]);
            assert!(parts.iter().any(|part| part.text == "Paste" && !part.enabled));
        });
    }
}
