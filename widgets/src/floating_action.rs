//! FloatingAction — the one action a screen exists for, as a round button
//! pinned to a corner or an edge of its container or of the window.
//!
//! When that action is really a small set, pressing the button fans the set
//! out toward the middle of the screen, along an arc, a row or a column, as
//! small round buttons with optional label chips. The plus on the button
//! turns into a cross, so the place that brought the set out also puts it
//! away again.
//!
//! The name is spelled out rather than abbreviated because the short prefix
//! already belongs to the property-panel controls, and a floating action
//! beside them would read as one of that family.
//!
//! # Toward the interior, always
//!
//! The anchor is the only thing that decides direction. A button pinned to
//! a corner can only grow into the quarter of the screen in front of it, and
//! one pinned to the middle of an edge only into the half in front of that
//! edge, so the anchor names both where the button sits and which way its
//! set goes. A set that could open outward would put its actions under the
//! window's edge on exactly the screens that need them most: the small ones.
//! Every piece of geometry below is a pure function of the anchor, the
//! layout and a handful of sizes, which is what lets the tests pin every
//! combination without a window.
//!
//! # Drawn on its own overlay
//!
//! The button and its set float over whatever the container holds, and a
//! list scrolling underneath must not carry them away or clip them. So both
//! are drawn on an overlay list of the widget's own. Pinned, the widget
//! claims no room at all in its parent; laid out `inline` (a toolbar's
//! floating slot), the button takes its room in the parent's flow like any
//! control and only the set floats.
//!
//! Drawn on top is not heard first. A closed button takes a press only
//! where nothing that hears events before it lies underneath, which is why
//! it is declared last in its container, and why one pinned to the window
//! belongs last in the window's body: pinned from deep inside a page, it
//! floats over the panels beside the page but they still hear the press.
//!
//! While the set is out the widget holds the sweep lock, so a press that
//! misses the set closes it and reaches nothing underneath, the pairing
//! every popup in this crate uses. The button and the actions hit-test with
//! plain `hits`, which the lock would turn away too, so the lock is lifted
//! around their own dispatch and taken again after, as the popover does.
//! A lock outlives the widget that took it when the widget is dropped while
//! its set is out (a live edit rebuilds the tree), and a lock nobody holds
//! turns every press in the window away for good; so a dropped widget
//! leaves its lock behind for the window to release on the next event.
//!
//! # Any number of actions
//!
//! A set is not always three. An arc grows its radius before two actions
//! would touch, their round faces or the squares a press is aimed at, and
//! when the grown arc would leave the window it is already as wide as its
//! anchor lets it open (a quarter from a corner, a half from the middle of
//! an edge), so the actions go on round a second arc further out, the inner
//! one filled first. The arcs also stay inside a furthest radius,
//! `radial_max_radius`, however much window is left: an action a long reach
//! from the button is a slow one to hit, so a set that would reach past it
//! goes on round further arcs inside it just as it does in a small window.
//! A set no arcs inside it can hold rests on the arcs of the least room
//! past it that does, as near the button as the spacing allows. A row or a
//! column that would leave the window folds
//! into a second line one step further in. Nothing scrolls and nothing
//! shrinks: a scrolling set hides the actions it exists to show, and a
//! smaller target is a worse target. Only a window too small for the set at
//! all is overrun, the choice every popup in the crate makes.
//!
//! # Motion
//!
//! Durations and easings are the theme's motion tokens, so a set moves the
//! way the rest of the theme does. Each action runs its own clock, out in
//! the order the actions were declared and back the other way round, and
//! the stagger between them shrinks for a large set so twelve open about as
//! quickly as three. The plus turns on the same eases and the same clock. A
//! set reopened on its way back turns round from wherever each action is.
//! Whatever an ease does on the way, and a spring goes past the place and
//! back, presses and arrow keys aim at where each action comes to rest.

use crate::{
    animator::{Animate, Ease},
    button::*,
    event::TouchState,
    makepad_derive_widget::*,
    makepad_draw::*,
    overlay_place::{claim_escape, orphan_sweep_locks, release_orphaned_sweep_locks, span_inboard},
    popover::FocusTrap,
    widget::*,
    widget_tree::CxWidgetExt,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // Not splatted: TopLeft is an alignment preset, and TopCenter and
    // BottomCenter are already exported bare by the popover's placements.
    // Written out as FloatingAnchor.BottomRight they cannot shadow anything.
    mod.widgets.FloatingAnchor = set_type_default() do #(FloatingAnchor::script_api(vm))
    mod.widgets.FloatingPin = set_type_default() do #(FloatingPin::script_api(vm))
    mod.widgets.SpeedDialLayout = set_type_default() do #(SpeedDialLayout::script_api(vm))
    mod.widgets.SpeedDialLabels = set_type_default() do #(SpeedDialLabels::script_api(vm))

    mod.widgets.DrawFloatingShadowBase = #(DrawFloatingShadow::script_component(vm))
    set_type_default() do #(DrawFloatingShadow::script_shader(vm)){
        ..mod.draw.DrawQuad

        // Rust writes both per disc, so they are plain literals.
        radius: 28.0
        weight: 1.0

        /** shadow ink */
        color: uniform(theme.color_elevation_3)
        /** how soft the shadow's edge is, in points 0..48 step 1 */
        blur: uniform(theme.elevation_3_radius)
        /** how far the shadow falls below the disc, in points 0..24 step 1 */
        offset_y: uniform(theme.elevation_3_offset_y)

        pixel: fn() {
            // The quad is centred on the disc and reaches past it by the
            // blur and the drop, so the middle of the quad is the middle of
            // the disc.
            let p = self.pos * self.rect_size - self.rect_size * 0.5 - vec2(0.0, self.offset_y)
            let d = length(p) - self.radius
            // The logistic curve standing in for the normal one, half way
            // down at the disc's edge: exact for a circle, where the
            // distance to the edge is the whole story.
            let sigma = max(self.blur * 0.5, 0.001)
            let v = 1.0 / (1.0 + exp(clamp(1.702 * d / sigma, -30.0, 30.0)))
            return Pal.premul(vec4(self.color.xyz, self.color.w * v * self.weight))
        }
    }

    mod.widgets.DrawFloatingChipBase = #(DrawFloatingChip::script_component(vm))
    set_type_default() do #(DrawFloatingChip::script_shader(vm)){
        ..mod.draw.DrawQuad

        /** the chip's fill */
        color: uniform(theme.color_inverse_surface)
        /** the chip's corner rounding 0..12 step 0.5 */
        radius: uniform(theme.radius_s)

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
            sdf.fill(self.color)
            return sdf.result
        }
    }

    mod.widgets.FloatingActionItemBase = #(FloatingActionItem::register_widget(vm))

    /** One action in a floating action's set: a small round button with an
     * optional label chip. The set places it; its own width and height are
     * not read. */
    mod.widgets.FloatingActionItem = set_type_default() do mod.widgets.FloatingActionItemBase{
        width: Fit
        height: Fit
        /** the words on the chip, and the item's text in the test tree */
        label: ""
        /** the action answers presses; a disabled one is drawn dimmed 0..1 step 1 */
        enabled: true
        /** the action is in the set at all; a hidden one leaves no gap 0..1 step 1 */
        visible: true
        /** space between the button's edge and the chip, in points 0..24 step 1 */
        chip_gap: 8.
        /** room between the chip's edge and its words */
        chip_padding: Inset{left: 10. right: 10. top: 4. bottom: 4.}

        // The shader declares these inputs, so they take plain values here.
        draw_shadow +: {
            color: theme.color_elevation_2
            blur: theme.elevation_2_radius
            offset_y: theme.elevation_2_offset_y
        }
        /** The label's background. The inverse pair contrasts with the page
         * in either scheme, which a label floating over arbitrary content
         * needs. */
        draw_chip +: {
            color: theme.color_inverse_surface
            radius: theme.radius_s
        }
        draw_chip_text +: {
            color: theme.color_inverse_on_surface
            text_style: theme.font_regular{
                /** chip type size 6..24 step 0.5 */
                font_size: theme.type_label_m_size
                line_spacing: 1.0
            }
        }

        /** the small round face; set its icon with `button +: {draw_icon +: {svg: ...}}` */
        button := ButtonFloatingSm{}
    }

    mod.widgets.FloatingActionBase = #(FloatingAction::register_widget(vm))

    /** The one action a screen exists for, pinned to a corner or an edge;
     * a set of actions behind it fans out toward the middle. Declare it as
     * the last child of its container, so it hears a press before the
     * content under it does. */
    mod.widgets.FloatingAction = set_type_default() do mod.widgets.FloatingActionBase{
        width: Fit
        height: Fit
        /** where it is pinned, and so which way its set opens */
        anchor: mod.widgets.FloatingAnchor.BottomRight
        /** the rect it is pinned inside: its container, or the window */
        pin_to: mod.widgets.FloatingPin.Container
        /** laid out in its parent's flow, as in a toolbar's floating slot; anchor still picks the direction 0..1 step 1 */
        inline: false
        /** distance from the edges the anchor touches; the other edges are not read */
        edge_margin: Inset{left: 16. top: 16. right: 16. bottom: 16.}
        /** how the set is laid out: along an arc, a row or a column */
        dial: mod.widgets.SpeedDialLayout.Radial
        /** which label chips show: Auto is Always for a column and Hot otherwise */
        labels: mod.widgets.SpeedDialLabels.Auto
        /** the main button's diameter, in points 40..96 step 4 */
        size: 56.
        /** each action's diameter, in points 24..64 step 2 */
        item_size: 40.
        /** space between neighbours, and between the button and the first action 0..32 step 1 */
        item_gap: 12.
        /** the least radius of the arc; it grows before neighbours would touch 48..200 step 2 */
        radial_radius: 88.
        /** the furthest radius of the arcs: a set that would reach past it goes on round more arcs inside it; 0 is no limit 0..400 step 2 */
        radial_max_radius: 200.
        /** picking an action puts the set away 0..1 step 1 */
        close_on_pick: true
        /** shows the set and keeps it out: no pointer grab, no outside or Escape close 0..1 step 1 */
        pinned: false
        /** each action's travel out, and the plus turning to the cross, in seconds 0..0.6 step 0.01 */
        enter_secs: theme.motion_short_4
        /** how the travel out and the turn ease: one of the theme's motion easings */
        enter_ease: theme.motion_ease_emphasized_decelerate
        /** each action's travel back under the button, and the turn back, in seconds 0..0.6 step 0.01 */
        exit_secs: theme.motion_short_3
        /** how the travel back and the turn back ease: one of the theme's motion easings */
        exit_ease: theme.motion_ease_standard_accelerate
        /** delay added per action, in seconds; shrunk for a large set so the last sets off within 0.15 of the first 0..0.1 step 0.005 */
        stagger_secs: 0.03
        /** no travel and no turn: everything lands at once 0..1 step 1 */
        reduced_motion: false
        /** drawn at all */
        visible: true

        draw_shadow +: {
            color: theme.color_elevation_3
            blur: theme.elevation_3_radius
            offset_y: theme.elevation_3_offset_y
        }

        /** the main face; its walk is rewritten from `size` every draw */
        main := ButtonFloating{}
    }

    /** A smaller floating action, for dense screens. */
    mod.widgets.FloatingActionSm = mod.widgets.FloatingAction{
        size: 40.
        item_size: 32.
        item_gap: 8.
        radial_radius: 64.
        radial_max_radius: 156.
        main +: {width: 40. height: 40.}
    }

    /** A larger floating action, for the one screen that is mostly this. */
    mod.widgets.FloatingActionLg = mod.widgets.FloatingAction{
        size: 72.
        item_size: 48.
        item_gap: 14.
        radial_radius: 108.
        radial_max_radius: 238.
        main +: {width: 72. height: 72.}
    }
}

/// Where a floating action is pinned. The same choice decides which way its
/// set opens: always into the container, away from the edges it touches.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum FloatingAnchor {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    CenterRight,
    BottomLeft,
    BottomCenter,
    #[pick]
    #[default]
    BottomRight,
}

impl FloatingAnchor {
    pub const ALL: [FloatingAnchor; 8] = [
        FloatingAnchor::TopLeft,
        FloatingAnchor::TopCenter,
        FloatingAnchor::TopRight,
        FloatingAnchor::CenterLeft,
        FloatingAnchor::CenterRight,
        FloatingAnchor::BottomLeft,
        FloatingAnchor::BottomCenter,
        FloatingAnchor::BottomRight,
    ];

    /// Which edges the anchor touches: -1 for the left or top edge, 1 for the
    /// right or bottom one, and 0 on an axis where it is centred instead.
    pub fn sides(self) -> (f64, f64) {
        match self {
            FloatingAnchor::TopLeft => (-1.0, -1.0),
            FloatingAnchor::TopCenter => (0.0, -1.0),
            FloatingAnchor::TopRight => (1.0, -1.0),
            FloatingAnchor::CenterLeft => (-1.0, 0.0),
            FloatingAnchor::CenterRight => (1.0, 0.0),
            FloatingAnchor::BottomLeft => (-1.0, 1.0),
            FloatingAnchor::BottomCenter => (0.0, 1.0),
            FloatingAnchor::BottomRight => (1.0, 1.0),
        }
    }

    /// The anchor as the test tree spells it.
    pub fn slug(self) -> &'static str {
        match self {
            FloatingAnchor::TopLeft => "top-left",
            FloatingAnchor::TopCenter => "top-center",
            FloatingAnchor::TopRight => "top-right",
            FloatingAnchor::CenterLeft => "center-left",
            FloatingAnchor::CenterRight => "center-right",
            FloatingAnchor::BottomLeft => "bottom-left",
            FloatingAnchor::BottomCenter => "bottom-center",
            FloatingAnchor::BottomRight => "bottom-right",
        }
    }

    /// A corner, rather than the middle of an edge.
    pub fn is_corner(self) -> bool {
        let (sx, sy) = self.sides();
        sx != 0.0 && sy != 0.0
    }
}

/// The rect a floating action is pinned inside.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum FloatingPin {
    /// The container it is declared in.
    #[pick]
    #[default]
    Container,
    /// The whole window, whatever it is declared in.
    Window,
}

/// How a floating action's set is laid out.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum SpeedDialLayout {
    /// Along an arc: a quarter circle from a corner, a half from an edge.
    #[pick]
    #[default]
    Radial,
    /// Along a row.
    Horizontal,
    /// Along a column.
    Vertical,
}

impl SpeedDialLayout {
    pub fn slug(self) -> &'static str {
        match self {
            SpeedDialLayout::Radial => "radial",
            SpeedDialLayout::Horizontal => "horizontal",
            SpeedDialLayout::Vertical => "vertical",
        }
    }
}

/// Which label chips a floating action's set shows.
#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook, Default)]
pub enum SpeedDialLabels {
    /// Always for a column, whose chips line up beside it; Hot for an arc
    /// or a row, whose chips would overlap their neighbours.
    #[pick]
    #[default]
    Auto,
    /// Every chip.
    Always,
    /// The chip of the action under the pointer or with key focus.
    Hot,
    /// None: the icons carry the meaning.
    Never,
}

/// Which side of its action a label chip goes on.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum ChipSide {
    Left,
    Right,
    Above,
    Below,
    /// Straight out from the main button, past the action.
    #[default]
    Outward,
}

/// What a floating action reports.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum FloatingActionAction {
    /// The button was clicked and there is no set behind it: the action
    /// itself.
    Pressed,
    /// The set came out.
    Opened,
    /// The set was put away, however: the button, a pick, Escape, a press
    /// outside, or the window losing focus.
    Closed,
    /// An action was picked, by the child id it was declared under.
    Picked(LiveId),
    #[default]
    None,
}

/// Inset kept between a chip and the window's edges.
const EDGE: f64 = 6.0;
/// An arrow key moves to an action within this many degrees either side of
/// the arrow's direction. Wider than 45 so that the diagonal step of a
/// quarter arc is reachable with either of its two arrows.
const ARROW_CONE_DEGREES: f64 = 67.5;
/// Two actions this close in distance are a tie, settled by alignment. The
/// ends of an arc are the same distance from its middle to within floating
/// point, and the better aligned one is the one the arrow means.
const ARROW_TIE: f64 = 0.5;
/// The share of its size an action starts its travel at.
const START_SCALE: f64 = 0.4;
/// How dark an action's shadow is against the main button's. Smaller things
/// sit lower.
const ITEM_SHADOW_WEIGHT: f32 = 0.6;
/// A rect read on the event side counts as moved past this many points.
const HONEST_SLACK: f64 = 0.5;
/// The most the stagger adds up to across a whole set, in seconds. Past a
/// handful of actions a fixed stagger makes the last one arrive long after
/// the hand has moved on, so a large set shares this out instead.
const STAGGER_BUDGET: f64 = 0.15;
/// A window with no edges anywhere near, for the layout nothing bounds.
const UNBOUNDED: Rect = Rect {
    pos: DVec2 { x: -1.0e9, y: -1.0e9 },
    size: DVec2 { x: 2.0e9, y: 2.0e9 },
};

/// The unit vector from the anchor into its container.
pub fn interior(anchor: FloatingAnchor) -> DVec2 {
    let (sx, sy) = anchor.sides();
    dvec2(-sx, -sy).normalize()
}

/// The main button's rect inside `bounds`, `margin` in from the edges the
/// anchor touches and centred on an axis it does not touch.
///
/// Each axis is then pulled inside the bounds, so a container smaller than
/// the button keeps the button's low edge in it rather than hanging the
/// button off the container's top or left.
pub fn main_rect(anchor: FloatingAnchor, bounds: Rect, size: f64, margin: Inset) -> Rect {
    let (sx, sy) = anchor.sides();
    let place = |side: f64, lo: f64, extent: f64, near: f64, far: f64| -> f64 {
        let start = if side < 0.0 {
            lo + near
        } else if side > 0.0 {
            lo + extent - far - size
        } else {
            lo + (extent - size) * 0.5
        };
        span_inboard(start, size, lo, extent)
    };
    Rect {
        pos: dvec2(
            place(sx, bounds.pos.x, bounds.size.x, margin.left, margin.right),
            place(sy, bounds.pos.y, bounds.size.y, margin.top, margin.bottom),
        ),
        size: dvec2(size, size),
    }
}

/// The part of the window a pass of `pass` leaves usable after the safe-area
/// insets.
fn safe_rect(pass: DVec2, insets: SafeAreaInsets) -> Rect {
    Rect {
        pos: dvec2(insets.left, insets.top),
        size: dvec2(
            (pass.x - insets.left - insets.right).max(0.0),
            (pass.y - insets.top - insets.bottom).max(0.0),
        ),
    }
}

/// The rect a floating action pins inside: its host, less whatever of the
/// host the safe-area insets cover. A button under a notch or a home
/// indicator is a button that cannot be pressed.
pub fn pin_bounds(host: Rect, pass: DVec2, insets: SafeAreaInsets) -> Rect {
    let safe = safe_rect(pass, insets);
    let x0 = host.pos.x.max(safe.pos.x);
    let y0 = host.pos.y.max(safe.pos.y);
    let x1 = (host.pos.x + host.size.x).min(safe.pos.x + safe.size.x);
    let y1 = (host.pos.y + host.size.y).min(safe.pos.y + safe.size.y);
    Rect {
        pos: dvec2(x0, y0),
        size: dvec2((x1 - x0).max(0.0), (y1 - y0).max(0.0)),
    }
}

/// The rect a chip must stay inside: the safe part of the window, a little
/// further in, as every popup in the crate keeps.
fn window_bounds(pass: DVec2, insets: SafeAreaInsets) -> Rect {
    let safe = safe_rect(pass, insets);
    Rect {
        pos: safe.pos + dvec2(EDGE, EDGE),
        size: dvec2((safe.size.x - EDGE * 2.0).max(0.0), (safe.size.y - EDGE * 2.0).max(0.0)),
    }
}

/// The arc a radial set is laid along: where it starts, which way it runs
/// (+1 clockwise, -1 anticlockwise) and how far. Radians, clockwise from
/// straight up, the measure the pie menu's ring uses.
///
/// The two ends lie along the two edges the arc runs between, so a corner
/// set reads as a quarter circle and an edge set as a half, both opening
/// into the container.
pub fn radial_arc(anchor: FloatingAnchor) -> (f64, f64, f64) {
    let (start, dir, span): (f64, f64, f64) = match anchor {
        FloatingAnchor::BottomRight => (0.0, -1.0, 90.0),
        FloatingAnchor::BottomLeft => (0.0, 1.0, 90.0),
        FloatingAnchor::TopRight => (180.0, 1.0, 90.0),
        FloatingAnchor::TopLeft => (180.0, -1.0, 90.0),
        FloatingAnchor::CenterRight => (0.0, -1.0, 180.0),
        FloatingAnchor::CenterLeft => (0.0, 1.0, 180.0),
        FloatingAnchor::BottomCenter => (270.0, 1.0, 180.0),
        FloatingAnchor::TopCenter => (270.0, -1.0, 180.0),
    };
    (start.to_radians(), dir, span.to_radians())
}

/// How far out an arc of `n` actions spanning `span` radians is laid.
///
/// Never less than `min_r`, never so close that the first action touches
/// the main button, and never so tight that two neighbours' discs, `gap`
/// apart, would overlap along the chord between them. The arc grows rather
/// than the actions shrinking: a smaller target is a worse target.
pub fn radial_radius(n: usize, span: f64, item: f64, gap: f64, main: f64, min_r: f64) -> f64 {
    let clear_of_main = main * 0.5 + gap + item * 0.5;
    let mut r = min_r.max(clear_of_main);
    if n >= 2 {
        let step = span / (n - 1) as f64;
        let half_sin = (step * 0.5).sin();
        if half_sin > 1e-9 {
            r = r.max((item + gap) / (2.0 * half_sin));
        }
    }
    r
}

/// How far out an arc of `n` actions spanning `span` radians is laid so
/// that, on top of [`radial_radius`], no two actions' squares overlap and
/// none overlaps the main button's square.
///
/// The chord rule keeps round faces a gap apart, but a press is aimed at an
/// action's square, and two neighbours a diagonal step apart have squares
/// that reach toward each other past their faces: at a gap under (√2 - 1)
/// times the diameter they share a corner. Two squares are clear when their
/// middles are a diameter apart along one axis or the other, and the longer
/// axis of a step between two places on the arc is that step's length
/// times the larger of the sine and cosine of the angle half way between
/// them. Every arc starts along an axis ([`radial_arc`]), so that share
/// depends only on the span and the count, never on the anchor.
pub fn arc_radius(n: usize, span: f64, item: f64, gap: f64, main: f64, min_r: f64) -> f64 {
    let mut r = radial_radius(n, span, item, gap, main, min_r);
    let angle = |k: usize| {
        if n >= 2 {
            span * k as f64 / (n - 1) as f64
        } else {
            span * 0.5
        }
    };
    let longer_axis = |a: f64| a.sin().abs().max(a.cos().abs());
    for k in 0..n {
        let a = angle(k);
        r = r.max((main + item) * 0.5 / longer_axis(a));
        for j in k + 1..n {
            let b = angle(j);
            let apart = 2.0 * ((b - a) * 0.5).sin() * longer_axis((a + b) * 0.5);
            if apart > 1e-9 {
                r = r.max(item / apart);
            }
        }
    }
    r
}

/// How far apart two arcs are laid: far enough that a face on one is a gap
/// clear of a face on the other whatever their angles, and a square on one
/// clear of a square on the other, which along a diagonal takes √2 times
/// the diameter.
pub fn arc_step(item: f64, gap: f64) -> f64 {
    (item + gap).max(item * std::f64::consts::SQRT_2)
}

/// Where each of `n` actions sits, as an offset from the main button's
/// centre, in the order they were declared, when nothing is in the way: the
/// layout a window large enough for the whole set gets. An arc still keeps
/// inside `max_r` when that is above 0, as [`arc_counts`] lays it out.
///
/// A row or a column runs from the button toward the interior along its own
/// axis when that axis leads inward. When it does not (a row from the
/// middle of the top or bottom edge, a column from the middle of a side) it
/// is centred on the button instead, one step toward the interior: running
/// along the edge in one direction would pick a side for no reason.
pub fn dial_offsets(
    anchor: FloatingAnchor,
    dial: SpeedDialLayout,
    n: usize,
    main: f64,
    item: f64,
    gap: f64,
    min_r: f64,
    max_r: f64,
) -> Vec<DVec2> {
    dial_plan(anchor, dial, n, main, item, gap, min_r, max_r, dvec2(0.0, 0.0), UNBOUNDED).offsets
}

/// A set laid out inside a window: where each action rests, and which arc
/// or line it rests on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DialPlan {
    /// Each action's resting offset from the main button's centre, in
    /// declaration order.
    pub offsets: Vec<DVec2>,
    /// The arc or line each action is on, 0 nearest the button. It never
    /// goes down along the order: the inner arc or the first line fills
    /// first.
    pub rows: Vec<usize>,
    /// Each arc's radius, innermost first; empty for a row or a column.
    pub radii: Vec<f64>,
}

impl DialPlan {
    /// How many arcs or lines the set is laid on.
    pub fn row_count(&self) -> usize {
        self.rows.last().map_or(0, |row| row + 1)
    }
}

/// The set laid out around a main button centred at `centre`, every action
/// inside `window` whenever the window can hold the set at all.
///
/// An arc grows with its count by the chord rule, and further so no two
/// squares meet, and once it would leave the window, or reach past `max_r`
/// when that is above 0, goes on round further arcs a step apart, the inner
/// ones filled first. A row or a column folds
/// into parallel lines a step further in. A window that cannot hold the set
/// is overrun rather than the set being cut: an action that cannot be
/// reached is worse than one partly under the window's edge.
///
/// `max_r` is measured from `centre`, the button's middle, even when a
/// small button close to the window's edge moves its set in.
pub fn dial_plan(
    anchor: FloatingAnchor,
    dial: SpeedDialLayout,
    n: usize,
    main: f64,
    item: f64,
    gap: f64,
    min_r: f64,
    max_r: f64,
    centre: DVec2,
    window: Rect,
) -> DialPlan {
    let lay_out = |centre: DVec2, max_r: f64| match dial {
        SpeedDialLayout::Radial => radial_plan(anchor, n, main, item, gap, min_r, max_r, centre, window),
        _ => line_plan(anchor, dial, n, main, item, gap, centre, window),
    };
    let plan = lay_out(centre, max_r);
    // The actions level with the button, the ends of an arc and the first
    // line of a row or a column, reach half an action past its middle
    // toward the edges it is pinned to. A small button close to the window's
    // edge has less room than that, and the whole set moves in by what they
    // overrun, which keeps its shape and moves no action nearer the button.
    let shift = pinned_overrun(anchor, &plan.offsets, centre, item, window);
    if shift.x == 0.0 && shift.y == 0.0 {
        return plan;
    }
    // The arcs are laid around the moved centre, so they are held that much
    // nearer for no action to rest past the furthest radius from the
    // button's own middle.
    let held = if max_r > 0.0 {
        (max_r - shift.length()).max(f64::MIN_POSITIVE)
    } else {
        max_r
    };
    let mut plan = lay_out(centre + shift, held);
    for offset in &mut plan.offsets {
        *offset += shift;
    }
    plan
}

/// How far a set laid out around `centre` has to move away from the edges
/// its anchor touches for none of its actions to lie past them in
/// `window`. Never toward an edge, and never more than half an action,
/// which is all an action level with the button reaches past its middle.
fn pinned_overrun(anchor: FloatingAnchor, offsets: &[DVec2], centre: DVec2, item: f64, window: Rect) -> DVec2 {
    let (sx, sy) = anchor.sides();
    let half = item * 0.5;
    let axis = |side: f64, middle: f64, lo: f64, extent: f64, along: fn(DVec2) -> f64| -> f64 {
        if side == 0.0 {
            return 0.0;
        }
        let over = offsets
            .iter()
            .map(|offset| {
                let at = middle + along(*offset);
                if side > 0.0 {
                    at + half - (lo + extent)
                } else {
                    lo - (at - half)
                }
            })
            .fold(0.0, f64::max);
        if over > 1e-9 {
            -side * over.min(half)
        } else {
            0.0
        }
    };
    dvec2(
        axis(sx, centre.x, window.pos.x, window.size.x, |offset| offset.x),
        axis(sy, centre.y, window.pos.y, window.size.y, |offset| offset.y),
    )
}

fn radial_plan(
    anchor: FloatingAnchor,
    n: usize,
    main: f64,
    item: f64,
    gap: f64,
    min_r: f64,
    max_r: f64,
    centre: DVec2,
    window: Rect,
) -> DialPlan {
    let (start, dir, span) = radial_arc(anchor);
    let counts = arc_counts(anchor, n, main, item, gap, min_r, max_r, centre, window);
    let mut plan = DialPlan {
        radii: arc_radii(&counts, span, item, gap, main, min_r),
        ..DialPlan::default()
    };
    for (row, &count) in counts.iter().enumerate() {
        let r = plan.radii[row];
        for i in 0..count {
            let a = if count >= 2 {
                start + dir * span * i as f64 / (count - 1) as f64
            } else {
                start + dir * span * 0.5
            };
            plan.offsets.push(dvec2(a.sin() * r, -a.cos() * r));
            plan.rows.push(row);
        }
    }
    plan
}

/// Each arc's radius, innermost first, for arcs holding `counts` actions:
/// each as tight as its own count lets it be, and a whole arc step clear of
/// the arc inside it, so neither a face nor a square on one touches one on
/// the other whatever their angles.
fn arc_radii(counts: &[usize], span: f64, item: f64, gap: f64, main: f64, min_r: f64) -> Vec<f64> {
    let mut radii: Vec<f64> = Vec::with_capacity(counts.len());
    for &count in counts {
        let least = arc_radius(count, span, item, gap, main, min_r);
        radii.push(match radii.last() {
            Some(inner) => least.max(inner + arc_step(item, gap)),
            None => least,
        });
    }
    radii
}

/// How many of `n` actions each arc holds, innermost first.
///
/// One arc whenever one fits. Otherwise the outermost arc goes as far out
/// as the room allows, as few arcs as the set needs go inside it a step
/// apart, and each takes all the spacing lets it hold before the next one
/// out takes any: the actions nearest the button are the ones reached
/// first, and a half-empty inner arc spends its room on nothing. The room
/// is what the window leaves, and no more than `max_r` when that is above
/// 0: left to the window alone one arc grows until the far actions are a
/// long reach from the button.
///
/// A set no number of arcs inside the room can hold is laid out as it
/// would be in the least room further out that does hold it: no action is
/// made smaller, none is left out, and none rests further from the button
/// than the set needs. Past `max_r` that room still keeps inside the
/// window whenever the window can hold the set. A window that cannot gets
/// arcs from the least radius out, as tight as they go, and they overrun
/// it.
pub fn arc_counts(
    anchor: FloatingAnchor,
    n: usize,
    main: f64,
    item: f64,
    gap: f64,
    min_r: f64,
    max_r: f64,
    centre: DVec2,
    window: Rect,
) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let (_, _, span) = radial_arc(anchor);
    let window_room = arc_room(anchor, centre, item, window);
    let room = window_room.min(furthest_radius(max_r));
    if let Some(counts) = arcs_within(n, span, main, item, gap, min_r, room) {
        return counts;
    }
    let least = least_room(n, span, main, item, gap, min_r, room);
    if least <= window_room + 1e-9 {
        if let Some(counts) = arcs_within(n, span, main, item, gap, min_r, least) {
            return counts;
        }
    }
    let step = arc_step(item, gap);
    let mut counts = Vec::new();
    let mut left = n;
    let mut r = arc_radius(1, span, item, gap, main, min_r);
    while left > 0 {
        let held = arc_capacity(r, span, item, gap, main, min_r, left);
        counts.push(held);
        left -= held;
        r += step;
    }
    counts
}

/// The least room past `room` inside which some number of arcs holds all
/// `n` actions.
///
/// An arc's capacity only changes where its radius reaches the least
/// radius of an arc of some count, and the arcs inside a room lie whole
/// steps in from it, so the least room is one of those radii a whole number
/// of steps further out. A set that fits in a room fits in any larger one,
/// so the least is the first of them, in order, that holds it. `n` arcs of
/// one a step apart always do.
fn least_room(n: usize, span: f64, main: f64, item: f64, gap: f64, min_r: f64, room: f64) -> f64 {
    let step = arc_step(item, gap);
    let mut rooms: Vec<f64> = (1..=n)
        .map(|count| arc_radius(count, span, item, gap, main, min_r))
        .flat_map(|r| (0..n).map(move |k| r + k as f64 * step))
        .filter(|r| *r > room)
        .collect();
    rooms.sort_by(f64::total_cmp);
    let first = rooms.partition_point(|r| arcs_within(n, span, main, item, gap, min_r, *r).is_none());
    rooms.get(first).copied().unwrap_or(f64::INFINITY)
}

/// How many of `n` actions each arc holds with every arc inside `room`:
/// one arc when it fits, and otherwise the fewest arcs a step apart with
/// the outermost at `room`, each filled before the next one out takes any.
/// `None` when no number of arcs inside `room` holds the set.
fn arcs_within(n: usize, span: f64, main: f64, item: f64, gap: f64, min_r: f64, room: f64) -> Option<Vec<usize>> {
    if n == 1 || arc_radius(n, span, item, gap, main, min_r) <= room + 1e-9 {
        return Some(vec![n]);
    }
    let step = arc_step(item, gap);
    let least = arc_radius(1, span, item, gap, main, min_r);
    let mut arcs = 2;
    loop {
        let first = room - (arcs - 1) as f64 * step;
        if first < least - 1e-9 {
            return None;
        }
        let mut left = n;
        let mut counts = Vec::new();
        for k in 0..arcs {
            if left == 0 {
                break;
            }
            let held = arc_capacity(first + k as f64 * step, span, item, gap, main, min_r, left);
            counts.push(held);
            left -= held;
        }
        if left == 0 {
            return Some(counts);
        }
        arcs += 1;
    }
}

/// The furthest an arc may be laid for a furthest radius of `max_r`: that
/// radius, or no limit at all when it is 0.
fn furthest_radius(max_r: f64) -> f64 {
    if max_r > 0.0 {
        max_r
    } else {
        f64::INFINITY
    }
}

/// How many actions an arc of radius `r` spanning `span` holds end to end
/// with no two faces closer than `gap` and no two squares overlapping: at
/// least one, and at most `most`.
pub fn arc_capacity(r: f64, span: f64, item: f64, gap: f64, main: f64, min_r: f64, most: usize) -> usize {
    let mut held = 1;
    while held < most && arc_radius(held + 1, span, item, gap, main, min_r) <= r + 1e-9 {
        held += 1;
    }
    held
}

/// The largest radius an arc from `anchor` around `centre` can have with
/// every action on it inside `window`; infinite when nothing bounds it.
///
/// Read off the directions the arc covers rather than off the actions on
/// it, so the room an arc has does not change as actions are added to it.
pub fn arc_room(anchor: FloatingAnchor, centre: DVec2, item: f64, window: Rect) -> f64 {
    let (start, dir, span) = radial_arc(anchor);
    let covers = |a: f64| ((a - start) * dir).rem_euclid(std::f64::consts::TAU) <= span + 1e-9;
    // The arc reaches furthest toward an edge at one of its ends or where
    // it passes straight toward that edge.
    let mut angles = vec![start, start + dir * span];
    angles.extend(
        (0..4)
            .map(|quarter| quarter as f64 * std::f64::consts::FRAC_PI_2)
            .filter(|a| covers(*a)),
    );
    let half = item * 0.5;
    let sides = [
        (dvec2(1.0, 0.0), window.pos.x + window.size.x - centre.x),
        (dvec2(-1.0, 0.0), centre.x - window.pos.x),
        (dvec2(0.0, 1.0), window.pos.y + window.size.y - centre.y),
        (dvec2(0.0, -1.0), centre.y - window.pos.y),
    ];
    sides.iter().fold(f64::INFINITY, |room, (toward, space)| {
        let reach = angles
            .iter()
            .map(|a| a.sin() * toward.x - a.cos() * toward.y)
            .fold(f64::NEG_INFINITY, f64::max);
        if reach > 1e-9 {
            room.min((space - half) / reach)
        } else {
            room
        }
    })
}

/// A row or a column, folded into parallel lines when one line would leave
/// the window.
fn line_plan(
    anchor: FloatingAnchor,
    dial: SpeedDialLayout,
    n: usize,
    main: f64,
    item: f64,
    gap: f64,
    centre: DVec2,
    window: Rect,
) -> DialPlan {
    let d0 = main * 0.5 + gap + item * 0.5;
    let step = item + gap;
    let along_x = dial == SpeedDialLayout::Horizontal;
    let (run, across) = line_axes(anchor, dial);
    let per_line = line_capacity(run, along_x, centre, item, gap, d0, window)
        .unwrap_or(n)
        .max(1);
    let lines = n.div_ceil(per_line).max(1);
    let centred = |k: usize, count: usize| (k as f64 - (count as f64 - 1.0) * 0.5) * step;
    let mut plan = DialPlan::default();
    for i in 0..n {
        let line = i / per_line;
        let in_line = if line + 1 == lines { n - line * per_line } else { per_line };
        let along = if run != 0.0 {
            run * (d0 + (i % per_line) as f64 * step)
        } else {
            centred(i % per_line, in_line)
        };
        let off = if run == 0.0 {
            across * (d0 + line as f64 * step)
        } else if across != 0.0 {
            across * line as f64 * step
        } else {
            // From the middle of a side the lines have no edge to step away
            // from, so they are centred on the button like the line itself.
            centred(line, lines)
        };
        plan.offsets.push(if along_x { dvec2(along, off) } else { dvec2(off, along) });
        plan.rows.push(line);
    }
    plan
}

/// How many actions one line holds before it would leave `window`, or None
/// when not even one fits and the line is left to overrun.
fn line_capacity(run: f64, along_x: bool, centre: DVec2, item: f64, gap: f64, d0: f64, window: Rect) -> Option<usize> {
    let step = item + gap;
    let half = item * 0.5;
    let (c, lo, hi) = if along_x {
        (centre.x, window.pos.x, window.pos.x + window.size.x)
    } else {
        (centre.y, window.pos.y, window.pos.y + window.size.y)
    };
    // How many steps past the first action still fit.
    let further = if run != 0.0 {
        let space = if run < 0.0 { c - lo } else { hi - c };
        (space - d0 - half) / step
    } else {
        // Centred on the button: half the line goes either way.
        ((c - lo).min(hi - c) - half) * 2.0 / step
    };
    (further >= -1e-9).then(|| (further + 1e-9).floor() as usize + 1)
}

/// `run`, the way a row or a column runs from the button along its own
/// axis, 0 when it is centred on the button instead; and `across`, the way
/// further lines go, 0 when there is no edge to move away from across the
/// line.
fn line_axes(anchor: FloatingAnchor, dial: SpeedDialLayout) -> (f64, f64) {
    let inward = interior(anchor);
    if dial == SpeedDialLayout::Horizontal {
        (sign(inward.x), sign(inward.y))
    } else {
        (sign(inward.y), sign(inward.x))
    }
}

fn sign(v: f64) -> f64 {
    if v > 1e-9 {
        1.0
    } else if v < -1e-9 {
        -1.0
    } else {
        0.0
    }
}

/// Which side of its action a label chip goes on: always the side facing
/// the interior, so a chip never pokes out past the edge the set is pinned
/// to.
pub fn chip_side(anchor: FloatingAnchor, dial: SpeedDialLayout) -> ChipSide {
    let (sx, sy) = anchor.sides();
    match dial {
        SpeedDialLayout::Vertical => {
            if sx < 0.0 {
                ChipSide::Right
            } else {
                // Right anchors, and the centre of the top or bottom edge,
                // where either side is inside and the reading side wins.
                ChipSide::Left
            }
        }
        SpeedDialLayout::Horizontal => {
            if sy < 0.0 {
                ChipSide::Below
            } else {
                ChipSide::Above
            }
        }
        SpeedDialLayout::Radial => ChipSide::Outward,
    }
}

/// The labels setting with Auto resolved. A column's chips line up beside
/// it and never touch each other, so they can all show; an arc's or a
/// row's would run into their neighbours, so only the one being aimed at
/// shows.
pub fn labels_shown(labels: SpeedDialLabels, dial: SpeedDialLayout) -> SpeedDialLabels {
    match labels {
        SpeedDialLabels::Auto => match dial {
            SpeedDialLayout::Vertical => SpeedDialLabels::Always,
            _ => SpeedDialLabels::Hot,
        },
        other => other,
    }
}

/// The labels setting resolved for a set laid out on `rows` arcs or lines.
/// A row or a column folded into lines puts the next line a step away, where
/// a chip beside one line lies over the other, so a folded set shows only
/// the chip being aimed at even when every chip was asked for.
pub fn labels_shown_in(labels: SpeedDialLabels, dial: SpeedDialLayout, rows: usize) -> SpeedDialLabels {
    match labels_shown(labels, dial) {
        SpeedDialLabels::Always if rows > 1 && dial != SpeedDialLayout::Radial => SpeedDialLabels::Hot,
        other => other,
    }
}

/// Where a chip of `size` goes beside an action of diameter `item` centred
/// at `centre`, `gap` clear of the disc on `side`, then pulled inside
/// `window` on both axes.
///
/// `outward` is the action's offset from the main button, which is the
/// direction an Outward chip goes in: past the action, away from the
/// button, where nothing else in the set is. `beyond` carries the chip
/// that much further out on its side, past the arcs or lines beyond the
/// action's own, so it does not lie over them.
pub fn chip_rect(
    centre: DVec2,
    item: f64,
    gap: f64,
    size: DVec2,
    side: ChipSide,
    outward: DVec2,
    beyond: f64,
    window: Rect,
) -> Rect {
    let out = item * 0.5 + gap + beyond.max(0.0);
    let pos = match side {
        ChipSide::Left => dvec2(centre.x - out - size.x, centre.y - size.y * 0.5),
        ChipSide::Right => dvec2(centre.x + out, centre.y - size.y * 0.5),
        ChipSide::Above => dvec2(centre.x - size.x * 0.5, centre.y - out - size.y),
        ChipSide::Below => dvec2(centre.x - size.x * 0.5, centre.y + out),
        ChipSide::Outward => {
            let u = if outward.length() > 1e-9 { outward.normalize() } else { dvec2(0.0, -1.0) };
            // How far the chip's own box reaches along that direction from
            // its middle, so its nearest edge, not its middle, is `gap` out.
            let reach = u.x.abs() * size.x * 0.5 + u.y.abs() * size.y * 0.5;
            let mid = centre + u * (out + reach);
            mid - size * 0.5
        }
    };
    Rect {
        pos: dvec2(
            span_inboard(pos.x, size.x, window.pos.x, window.size.x),
            span_inboard(pos.y, size.y, window.pos.y, window.size.y),
        ),
        size,
    }
}

/// Which side of its action the chip of an action on arc or line `row` goes
/// on, and how far past the action's own arc or line it is carried.
///
/// On an arc the chip goes straight out, past the outermost arc. Beside a
/// single row or column it goes on the side facing the interior. Once a
/// row or a column has folded, the lines beside the action's own would be
/// under that chip, so it is carried out past them: past the last line on
/// the side the lines were added, or, for lines centred on the button, out
/// on the side of the set its own line is nearer, where fewer lines are in
/// the way.
pub fn chip_place(anchor: FloatingAnchor, dial: SpeedDialLayout, row: usize, plan: &DialPlan, item: f64, gap: f64) -> (ChipSide, f64) {
    let side = chip_side(anchor, dial);
    if dial == SpeedDialLayout::Radial {
        let outermost = plan.radii.last().copied().unwrap_or(0.0);
        let own = plan.radii.get(row).copied().unwrap_or(outermost);
        return (side, (outermost - own).max(0.0));
    }
    let rows = plan.row_count();
    if rows <= 1 {
        return (side, 0.0);
    }
    let step = item + gap;
    let (run, across) = line_axes(anchor, dial);
    let last = rows - 1;
    if run != 0.0 && across == 0.0 {
        // Centred lines run from the negative side, the side `chip_side`
        // names, to the positive one.
        if row * 2 <= last {
            (side, row as f64 * step)
        } else {
            (opposite(side), (last - row) as f64 * step)
        }
    } else {
        // The chip already faces the way further lines go.
        (side, last.saturating_sub(row) as f64 * step)
    }
}

fn opposite(side: ChipSide) -> ChipSide {
    match side {
        ChipSide::Left => ChipSide::Right,
        ChipSide::Right => ChipSide::Left,
        ChipSide::Above => ChipSide::Below,
        ChipSide::Below => ChipSide::Above,
        ChipSide::Outward => ChipSide::Outward,
    }
}

/// Whether any of `rect` lies within `radius` of `centre`.
pub fn covers_disc(rect: Rect, centre: DVec2, radius: f64) -> bool {
    // Not `clamp`, which panics on a rect with no size or a NaN edge, and a
    // panic in a draw cannot unwind out of every platform's window callback.
    let nearest = dvec2(
        centre.x.max(rect.pos.x).min(rect.pos.x + rect.size.x),
        centre.y.max(rect.pos.y).min(rect.pos.y + rect.size.y),
    );
    (nearest - centre).length() < radius - 1e-9
}

/// Where a chip goes: where [`chip_rect`] puts it, unless pulling it inside
/// `window` has put it over its own action's face, and a chip that hides
/// the action it names names nothing. Then it goes beside the face on
/// another side: across the way it was pulled, the side facing the interior
/// (`inward`) first, and back the other way last. The first of those that
/// covers none of `faces` (each a middle and a diameter, the action's own
/// among them) wins, or failing that the first that leaves its own face
/// clear.
pub fn place_chip(
    centre: DVec2,
    item: f64,
    gap: f64,
    size: DVec2,
    side: ChipSide,
    outward: DVec2,
    beyond: f64,
    inward: DVec2,
    window: Rect,
    faces: &[(DVec2, f64)],
) -> Rect {
    let own = |rect: Rect| covers_disc(rect, centre, item * 0.5);
    let first = chip_rect(centre, item, gap, size, side, outward, beyond, window);
    if !own(first) {
        return first;
    }
    let upright = match side {
        ChipSide::Above | ChipSide::Below => true,
        ChipSide::Left | ChipSide::Right => false,
        ChipSide::Outward => outward.y.abs() >= outward.x.abs(),
    };
    let away = |lean: f64, fallback: f64| if lean > 1e-9 { 1.0 } else if lean < -1e-9 { -1.0 } else { fallback };
    let tries = if upright {
        let (near, far) = if away(inward.x, away(outward.x, -1.0)) > 0.0 {
            (ChipSide::Right, ChipSide::Left)
        } else {
            (ChipSide::Left, ChipSide::Right)
        };
        let back = if side == ChipSide::Below || (side == ChipSide::Outward && outward.y > 0.0) {
            ChipSide::Above
        } else {
            ChipSide::Below
        };
        [near, far, back]
    } else {
        let (near, far) = if away(inward.y, away(outward.y, -1.0)) > 0.0 {
            (ChipSide::Below, ChipSide::Above)
        } else {
            (ChipSide::Above, ChipSide::Below)
        };
        let back = if side == ChipSide::Right || (side == ChipSide::Outward && outward.x > 0.0) {
            ChipSide::Left
        } else {
            ChipSide::Right
        };
        [near, far, back]
    };
    let beside: Vec<Rect> = tries
        .iter()
        .map(|side| chip_rect(centre, item, gap, size, *side, outward, 0.0, window))
        .filter(|rect| !own(*rect))
        .collect();
    let clear = |rect: &&Rect| !faces.iter().any(|(middle, d)| covers_disc(**rect, *middle, d * 0.5));
    beside.iter().find(clear).or(beside.first()).copied().unwrap_or(first)
}

/// The action an arrow key moves to from `from` (or from `origin`, the main
/// button, when no action has focus), given every action's position.
///
/// Only actions within the arrow's cone count. Of those the nearest wins,
/// and among near-ties the one better aligned with the arrow, then the
/// earlier one. Arrows follow the direction the actions went rather than
/// stepping through a list: in an arc, "next" has no direction, and the
/// hand already knows where the actions are. With no candidate, focus
/// stays where it is.
pub fn arrow_target(points: &[DVec2], from: Option<usize>, origin: DVec2, dir: DVec2) -> Option<usize> {
    let base = match from {
        Some(i) => *points.get(i)?,
        None => origin,
    };
    nearest_in_cone(
        base,
        points.iter().copied().enumerate().filter(|(j, _)| Some(*j) != from),
        dir,
    )
}

/// The action an arrow key moves to on an arc, given every action's
/// position in the order the arcs are filled: from the button, the one
/// `arrow_target` picks on the inner arc (its first `first_arc` points);
/// from an action, whichever of its two neighbours in that order the arrow
/// points toward, the one it points at more squarely when it points toward
/// both, and none when it points toward neither.
///
/// An arc is walked in order rather than by distance. Across two arcs the
/// nearest action the arrow's way is often on the other arc, and a walk
/// that hops by distance passes some actions by for good. In order, the
/// inner arc is walked to its end and then on round the outer one. Where
/// the inner arc ends along one edge and the outer one starts back along
/// the other, the step between them goes the other way from the steps
/// before it, and the arrow that takes it is one that points that way.
/// Every step can still be taken and taken back: of two different
/// directions, one of the four arrows always points toward the first and
/// more squarely at it than at the second.
pub fn arc_walk_target(points: &[DVec2], first_arc: usize, from: Option<usize>, origin: DVec2, dir: DVec2) -> Option<usize> {
    let Some(i) = from else {
        let inner = if first_arc == 0 { points.len() } else { first_arc.min(points.len()) };
        return arrow_target(&points[..inner], None, origin, dir);
    };
    let here = *points.get(i)?;
    // How squarely the arrow points at neighbour `j`, when it points
    // toward it at all.
    let toward = |j: Option<usize>| -> Option<(usize, f64)> {
        let j = j?;
        let v = points[j] - here;
        let len = v.length();
        if len < 1e-9 {
            return None;
        }
        let along = (v.x * dir.x + v.y * dir.y) / len;
        (along > 1e-9).then_some((j, along))
    };
    let before = toward(i.checked_sub(1));
    let after = toward((i + 1 < points.len()).then_some(i + 1));
    match (after, before) {
        (Some(a), Some(b)) if (a.1 - b.1).abs() <= 1e-9 => None,
        (Some(a), Some(b)) => Some(if a.1 > b.1 { a.0 } else { b.0 }),
        (Some(a), None) => Some(a.0),
        (None, Some(b)) => Some(b.0),
        (None, None) => None,
    }
}

/// Of `candidates`, the one an arrow along `dir` from `base` means: only
/// those within the arrow's cone count, the nearest wins, and among
/// near-ties the better aligned, then the earlier.
fn nearest_in_cone(base: DVec2, candidates: impl Iterator<Item = (usize, DVec2)>, dir: DVec2) -> Option<usize> {
    let cone = ARROW_CONE_DEGREES.to_radians().cos();
    let within: Vec<(usize, f64, f64)> = candidates
        .filter_map(|(j, p)| {
            let v = p - base;
            let len = v.length();
            if len < 1e-9 {
                return None;
            }
            let along = (v.x * dir.x + v.y * dir.y) / len;
            (along > cone).then_some((j, len, along))
        })
        .collect();
    let nearest = within.iter().map(|c| c.1).fold(f64::INFINITY, f64::min);
    within
        .iter()
        .filter(|c| c.1 <= nearest + ARROW_TIE)
        .fold(None::<(usize, f64, f64)>, |best, c| match best {
            // Candidates come in index order, so keeping the earlier one on
            // an exact tie is keeping the lower index.
            Some(b) if b.2 >= c.2 => Some(b),
            _ => Some(*c),
        })
        .map(|c| c.0)
}

/// How far action `i` has travelled out, `elapsed` seconds after a closed
/// set opened: 0 at the button, 1 in place.
///
/// Each action starts `stagger` later than the one before, so the set reads
/// as coming out in order rather than as one lump, and moves along `ease`,
/// one of the theme's motion easings. An ease that overshoots carries the
/// value past 1 and back, which is what choosing it asks for, so the value
/// is not clamped. With motion reduced everything is already in place.
pub fn enter_progress(i: usize, elapsed: f64, enter: f64, stagger: f64, ease: &Ease, reduced: bool) -> f64 {
    if reduced {
        return 1.0;
    }
    // The leg a closed set gives its action `i` when it opens.
    legs_to(&vec![0.0; i + 1], 1.0, enter, stagger)[i].value(elapsed, ease)
}

/// How far action `i` of `n` still is from the button, `elapsed` seconds
/// after an open set was put away: 1 in place, 0 back under the button.
///
/// The last action out is the first one back, so the set folds up the way
/// it came instead of the first action crossing the rest on its way in.
pub fn exit_progress(i: usize, n: usize, elapsed: f64, exit: f64, stagger: f64, ease: &Ease, reduced: bool) -> f64 {
    if reduced {
        return 0.0;
    }
    // The leg an open set gives its action `i` when it is put away.
    legs_to(&vec![1.0; n.max(i + 1)], 0.0, exit, stagger)[i].value(elapsed, ease)
}

/// `ease` at `x`, exact at both ends: some easings only come near their end
/// value, and an action that never quite lands keeps asking for frames.
fn eased(ease: &Ease, x: f64) -> f64 {
    if x <= 0.0 {
        0.0
    } else if x >= 1.0 {
        1.0
    } else {
        ease.map(x)
    }
}

/// The delay between one action setting off and the next, for a set of `n`:
/// the `stagger` asked for, shrunk when the whole set would otherwise take
/// more than [`STAGGER_BUDGET`] longer to open than one action does.
pub fn stagger_step(n: usize, stagger: f64) -> f64 {
    let stagger = stagger.max(0.0);
    if n < 2 {
        stagger
    } else {
        stagger.min(STAGGER_BUDGET / (n - 1) as f64)
    }
}

/// One stretch of an action's travel, or of the glyph's turn: from where it
/// was when the stretch began to where it ends, setting off after `delay`
/// and taking `secs`. Positions are shares of the way out, 0 under the
/// button and 1 in place.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Leg {
    pub from: f64,
    pub to: f64,
    pub delay: f64,
    pub secs: f64,
}

impl Leg {
    /// Already where it is going.
    pub fn at_rest(value: f64) -> Leg {
        Leg {
            from: value,
            to: value,
            delay: 0.0,
            secs: 0.0,
        }
    }

    /// Where the leg is `elapsed` seconds after the legs set off. Not clamped
    /// between its ends, for the ease that overshoots.
    pub fn value(&self, elapsed: f64, ease: &Ease) -> f64 {
        self.from + (self.to - self.from) * eased(ease, self.share(elapsed))
    }

    /// Whether the leg has arrived by `elapsed`.
    pub fn done(&self, elapsed: f64) -> bool {
        self.share(elapsed) >= 1.0
    }

    fn share(&self, elapsed: f64) -> f64 {
        let t = elapsed - self.delay;
        if self.secs <= 0.0 {
            if t >= 0.0 { 1.0 } else { 0.0 }
        } else {
            (t / self.secs).clamp(0.0, 1.0)
        }
    }
}

/// The legs a set's actions take toward `to` (1 out, 0 back) from `now`,
/// their positions at this moment, over `secs` and `step` apart.
///
/// From a set entirely at the other end they go in turn: out in the order
/// the actions were declared, back the other way round. From anywhere else,
/// a set reopened on its way back or put away while still coming out, every
/// action turns round at once from where it is and takes the share of
/// `secs` its distance asks for, so an action nearly home does not crawl
/// the last of the way and none waits in mid-air for its turn.
pub fn legs_to(now: &[f64], to: f64, secs: f64, step: f64) -> Vec<Leg> {
    let n = now.len();
    let origin = 1.0 - to;
    let fresh = now.iter().all(|v| (v - origin).abs() < 1e-9);
    now.iter()
        .enumerate()
        .map(|(i, &from)| {
            if fresh {
                let order = if to > 0.5 { i } else { n - 1 - i };
                Leg {
                    from,
                    to,
                    delay: order as f64 * step,
                    secs,
                }
            } else {
                Leg {
                    from,
                    to,
                    delay: 0.0,
                    secs: secs * (to - from).abs().min(1.0),
                }
            }
        })
        .collect()
}

/// A disc of `size` centred at `centre`, moved just far enough to lie
/// inside `room`. An ease that overshoots carries an action past its place,
/// and past the window's edge the overshoot would be cut off; it stops at
/// the edge instead. `room` includes the resting place, so a resting action
/// is never moved, even in a window too small for the set.
pub fn keep_inside(centre: DVec2, size: f64, room: Rect) -> DVec2 {
    let half = size * 0.5;
    dvec2(
        span_inboard(centre.x - half, size, room.pos.x, room.size.x) + half,
        span_inboard(centre.y - half, size, room.pos.y, room.size.y) + half,
    )
}

/// What a set pinned to a partly hidden container may draw in: where the
/// container's `visible` part stops on the sides that are hidden, and the
/// `window`'s edges on every other side.
///
/// The actions reach past their container by design, and an overshoot
/// further, so the container's own edges cut nothing; only an edge that
/// really hides the container, a scrolled view's, hides the set as well. A
/// container hidden entirely leaves nothing to draw in.
pub fn cut_sides(visible: Rect, host: Rect, window: Rect) -> Rect {
    if visible.size.x <= 0.0 || visible.size.y <= 0.0 {
        return Rect {
            pos: visible.pos,
            size: dvec2(0.0, 0.0),
        };
    }
    let (vx1, vy1) = (visible.pos.x + visible.size.x, visible.pos.y + visible.size.y);
    let x0 = if visible.pos.x > host.pos.x + HONEST_SLACK { visible.pos.x } else { window.pos.x };
    let y0 = if visible.pos.y > host.pos.y + HONEST_SLACK { visible.pos.y } else { window.pos.y };
    let x1 = if vx1 < host.pos.x + host.size.x - HONEST_SLACK { vx1 } else { window.pos.x + window.size.x };
    let y1 = if vy1 < host.pos.y + host.size.y - HONEST_SLACK { vy1 } else { window.pos.y + window.size.y };
    Rect {
        pos: dvec2(x0, y0),
        size: dvec2((x1 - x0).max(0.0), (y1 - y0).max(0.0)),
    }
}

/// What a set pinned to a partly hidden container may draw in this draw,
/// and the part of the container still on screen: `cut_sides` of what the
/// event side last read (`visible_then` of `host_then`), laid over the
/// container where this draw pins it.
///
/// The edges that hid the container then are the edges of a view it is
/// scrolled in, and a scroll moves the container under them without moving
/// them. So they stay where they were read. Were they carried along with
/// the container, every scroll step would draw the set once over whatever
/// sits beyond that view's edge before the next read put the cut back.
pub fn cut_now(visible_then: Rect, host_then: Rect, host_now: Rect, window: Rect) -> (Rect, Rect) {
    let open = cut_sides(visible_then, host_then, window);
    (intersect(host_now, open), open)
}

/// The part both rects cover, empty when they do not meet.
fn intersect(a: Rect, b: Rect) -> Rect {
    let x0 = a.pos.x.max(b.pos.x);
    let y0 = a.pos.y.max(b.pos.y);
    let x1 = (a.pos.x + a.size.x).min(b.pos.x + b.size.x);
    let y1 = (a.pos.y + a.size.y).min(b.pos.y + b.size.y);
    Rect {
        pos: dvec2(x0, y0),
        size: dvec2((x1 - x0).max(0.0), (y1 - y0).max(0.0)),
    }
}

fn contains_rect(outer: Rect, inner: Rect) -> bool {
    inner.pos.x >= outer.pos.x - HONEST_SLACK
        && inner.pos.y >= outer.pos.y - HONEST_SLACK
        && inner.pos.x + inner.size.x <= outer.pos.x + outer.size.x + HONEST_SLACK
        && inner.pos.y + inner.size.y <= outer.pos.y + outer.size.y + HONEST_SLACK
}

/// The smallest rect holding both.
fn bounding(a: Rect, b: Rect) -> Rect {
    let x0 = a.pos.x.min(b.pos.x);
    let y0 = a.pos.y.min(b.pos.y);
    let x1 = (a.pos.x + a.size.x).max(b.pos.x + b.size.x);
    let y1 = (a.pos.y + a.size.y).max(b.pos.y + b.size.y);
    Rect {
        pos: dvec2(x0, y0),
        size: dvec2(x1 - x0, y1 - y0),
    }
}

/// The widget's state as the test tree reads it: whether the set is out,
/// how it is laid out, and where it is pinned.
pub fn snapshot_text(open: bool, pinned: bool, dial: SpeedDialLayout, anchor: FloatingAnchor) -> String {
    let state = if pinned {
        "pinned"
    } else if open {
        "open"
    } else {
        "closed"
    };
    format!("{state} {} {}", dial.slug(), anchor.slug())
}

/// `rect` grown by `by` on every side.
fn grown(rect: Rect, by: f64) -> Rect {
    Rect {
        pos: rect.pos - by,
        size: rect.size + by * 2.0,
    }
}

fn near(a: Rect, b: Rect) -> bool {
    (a.pos.x - b.pos.x).abs() <= HONEST_SLACK
        && (a.pos.y - b.pos.y).abs() <= HONEST_SLACK
        && (a.size.x - b.size.x).abs() <= HONEST_SLACK
        && (a.size.y - b.size.y).abs() <= HONEST_SLACK
}

fn has_size(rect: Rect) -> bool {
    rect.size.x > 0.0 && rect.size.y > 0.0
}

/// Where Tab goes from `focus` among `stops`, in their order and round the
/// ends; from outside them, to the first, or with `back` to the last.
fn tab_target<T: Copy + PartialEq>(stops: &[T], focus: T, back: bool) -> Option<T> {
    let len = stops.len();
    if len == 0 {
        return None;
    }
    Some(match (stops.iter().position(|stop| *stop == focus), back) {
        (Some(i), false) => stops[(i + 1) % len],
        (Some(i), true) => stops[(i + len - 1) % len],
        (None, false) => stops[0],
        (None, true) => stops[len - 1],
    })
}

/// How far a shadow reaches past its disc: half as far again as its blur,
/// where the shade has faded to well under a level, plus its drop.
fn shadow_reach(draw: &DrawFloatingShadow, cx: &mut Cx) -> f64 {
    let mut blur = [0.0f32];
    let mut drop = [0.0f32];
    draw.get_uniform(cx, live_id!(blur), &mut blur);
    draw.get_uniform(cx, live_id!(offset_y), &mut drop);
    (blur[0] as f64 * 1.5 + (drop[0] as f64).abs()).max(2.0)
}

/// Read the named children of a widget out of the object it was applied
/// with, building the new ones and re-applying the ones it has.
///
/// Only named children are read: an action is reported by the id it was
/// declared under, and an anonymous one would have nothing to report.
fn apply_children(
    vm: &mut ScriptVm,
    apply: &Apply,
    scope: &mut Scope,
    value: ScriptValue,
    children: &mut Vec<(LiveId, WidgetRef)>,
    order: &mut Vec<LiveId>,
) {
    if !apply.is_eval() {
        if let Some(object) = value.as_object() {
            vm.vec_with(object, |vm, values| {
                for value in values {
                    let Some(id) = value.key.as_id() else {
                        continue;
                    };
                    if !WidgetRef::value_is_newable_widget(vm, value.value) {
                        continue;
                    }
                    if apply.is_reload() {
                        order.push(id);
                    }
                    if let Some((_, child)) = children.iter_mut().find(|(child_id, _)| *child_id == id) {
                        child.script_apply(vm, apply, scope, value.value);
                    } else {
                        children.push((id, WidgetRef::script_from_value_scoped(vm, scope, value.value)));
                    }
                }
            });
        }
    }
    // A reload rewrites the declaration order, and the declaration order is
    // the order the set is laid out in.
    if apply.is_reload() && (!order.is_empty() || children.is_empty()) {
        for (index, id) in order.iter().enumerate() {
            if let Some(position) = children.iter().position(|(old, _)| old == id) {
                children.swap(index, position);
            }
        }
        children.truncate(order.len());
    }
}

/// A soft disc under a round face.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFloatingShadow {
    #[deref]
    draw_super: DrawQuad,
    /// The disc's radius, in points.
    #[live]
    pub radius: f32,
    /// How dark, as a share of the shadow's ink.
    #[live]
    pub weight: f32,
}

/// The rounded box behind a label chip.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFloatingChip {
    #[deref]
    draw_super: DrawQuad,
}

/// Where the set puts an action this draw. Written by the floating action
/// before it draws the action, because only the set knows where its
/// actions go.
#[derive(Clone, Copy, Debug, Default)]
struct ItemPlace {
    centre: DVec2,
    size: f64,
    side: ChipSide,
    outward: DVec2,
    beyond: f64,
    /// The way into the container, for a chip that has to find another
    /// side of its action.
    inward: DVec2,
    window: Rect,
    show_chip: bool,
}

/// One action in a floating action's set.
#[derive(Script, WidgetRef, WidgetSet, WidgetRegister)]
pub struct FloatingActionItem {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[live]
    draw_shadow: DrawFloatingShadow,
    #[live]
    draw_chip: DrawFloatingChip,
    #[live]
    draw_chip_text: DrawText,

    /// The words on the chip.
    #[live]
    pub label: String,
    /// Whether the action answers presses.
    #[live(true)]
    pub enabled: bool,
    /// Whether the action is in the set at all. A hidden action is left out
    /// of the layout, so the set closes up rather than keeping its place.
    #[live(true)]
    pub visible: bool,
    #[live(8.0)]
    pub chip_gap: f64,
    #[live]
    pub chip_padding: Inset,

    #[rust]
    children: Vec<(LiveId, WidgetRef)>,
    #[rust]
    update_order: Vec<LiveId>,
    /// The `button :=` child.
    #[rust]
    button: WidgetRef,
    #[rust]
    place: ItemPlace,
    /// The chip drawn last, window-absolute, so a press on it is not read
    /// as a press outside the set.
    #[rust]
    chip_rect: Option<Rect>,
    /// The list of the set the action is in, written by the set. The set
    /// lays its actions out, so a change to one is a change to the set; and
    /// an action never drawn has no area of its own to redraw from.
    #[rust]
    owner_list: Option<DrawListId>,
}

impl ScriptHook for FloatingActionItem {
    fn on_before_apply(&mut self, _vm: &mut ScriptVm, apply: &Apply, _scope: &mut Scope, _value: ScriptValue) {
        if !apply.is_eval() {
            self.update_order.clear();
        }
    }

    fn on_after_apply(&mut self, vm: &mut ScriptVm, apply: &Apply, scope: &mut Scope, value: ScriptValue) {
        apply_children(vm, apply, scope, value, &mut self.children, &mut self.update_order);
        self.button = self
            .children
            .iter()
            .find(|(id, _)| *id == live_id!(button))
            .map(|(_, child)| child.clone())
            .unwrap_or_default();
        let uid = self.uid;
        let enabled = self.enabled;
        let button = self.button.clone();
        let owner = self.owner_list;
        vm.with_cx_mut(|cx| {
            cx.widget_tree_mark_dirty(uid);
            // `enabled` is the item's to say, and the button is what has to
            // refuse the press and draw itself dimmed.
            sync_enabled(cx, &button, enabled);
            // A change to `visible` changes the whole set's layout.
            redraw_set(cx, owner, &button);
        });
    }
}

/// Redraw the set an action is in: its list when the set has said which,
/// and otherwise the list the action's button was last drawn in, which is
/// the set's too.
fn redraw_set(cx: &mut Cx, owner: Option<DrawListId>, button: &WidgetRef) {
    match owner {
        Some(list) => cx.redraw_list(list),
        None => button.redraw(cx),
    }
}

fn sync_enabled(cx: &mut Cx, button: &WidgetRef, enabled: bool) {
    if button.is_empty() {
        return;
    }
    // Two switches: `enabled` is what refuses the press and picks the
    // cursor, the disabled track is what draws the face dimmed.
    button.as_button().set_enabled(cx, enabled);
    button.set_disabled(cx, !enabled);
}

impl WidgetNode for FloatingActionItem {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.button.area()
    }

    fn redraw(&mut self, cx: &mut Cx) {
        redraw_set(cx, self.owner_list, &self.button);
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        if !self.button.is_empty() {
            visit(live_id!(button), self.button.clone());
        }
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        self.button.find_widgets_from_point(cx, point, found);
    }

    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        if self.visible != visible {
            self.visible = visible;
            redraw_set(cx, self.owner_list, &self.button);
        }
    }

    fn visible(&self) -> bool {
        self.visible
    }
}

impl FloatingActionItem {
    pub fn set_label(&mut self, cx: &mut Cx, label: &str) {
        if self.label != label {
            self.label = label.to_string();
            redraw_set(cx, self.owner_list, &self.button);
        }
    }

    pub fn set_enabled(&mut self, cx: &mut Cx, enabled: bool) {
        if self.enabled != enabled {
            self.enabled = enabled;
            sync_enabled(cx, &self.button, enabled);
        }
    }

    /// The shadow and the button, where the set placed them this draw.
    fn draw_face(&mut self, cx: &mut Cx2d, scope: &mut Scope) {
        let place = self.place;
        let d = place.size.max(1.0);
        let half = d * 0.5;
        let face = Rect {
            pos: place.centre - half,
            size: dvec2(d, d),
        };

        let reach = shadow_reach(&self.draw_shadow, cx);
        self.draw_shadow.radius = half as f32;
        self.draw_shadow.weight = ITEM_SHADOW_WEIGHT;
        self.draw_shadow.draw_abs(cx, grown(face, reach));

        let _ = self.button.draw_walk(cx, scope, Walk::fixed(d, d).with_abs_pos(face.pos));
        self.chip_rect = None;
    }

    /// The label chip, when the set asked for one. The set draws every
    /// action's face before any chip, so a chip is never drawn under a
    /// neighbouring face; `faces` are where those faces rest, the button's
    /// among them, for a chip that has to find a side clear of them.
    fn draw_label_chip(&mut self, cx: &mut Cx2d, faces: &[(DVec2, f64)]) {
        let place = self.place;
        self.chip_rect = None;
        if !place.show_chip || self.label.is_empty() {
            return;
        }
        let d = place.size.max(1.0);
        let label = self.label.clone();
        let laid = self
            .draw_chip_text
            .layout(cx, 0.0, 0.0, None, false, Align::default(), &label);
        let scale = self.draw_chip_text.font_scale as f64;
        let text = dvec2(
            laid.size_in_lpxs.width as f64 * scale,
            laid.size_in_lpxs.height as f64 * scale,
        );
        let pad = self.chip_padding;
        let size = dvec2(text.x + pad.left + pad.right, text.y + pad.top + pad.bottom);
        let rect = place_chip(
            place.centre,
            d,
            self.chip_gap,
            size,
            place.side,
            place.outward,
            place.beyond,
            place.inward,
            place.window,
            faces,
        );
        self.draw_chip.draw_abs(cx, rect);
        self.draw_chip_text.draw_abs(cx, rect.pos + dvec2(pad.left, pad.top), &label);
        self.chip_rect = Some(rect);
    }
}

impl Widget for FloatingActionItem {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.button.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        self.draw_face(cx, scope);
        let own = [(self.place.centre, self.place.size)];
        self.draw_label_chip(cx, &own);
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.label.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.set_label(cx, v);
    }
}

impl FloatingActionItemRef {
    pub fn set_label(&self, cx: &mut Cx, label: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_label(cx, label);
        }
    }

    pub fn set_enabled(&self, cx: &mut Cx, enabled: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_enabled(cx, enabled);
        }
    }
}

/// The one action a screen exists for, and the set behind it.
#[derive(Script, WidgetRef, WidgetSet, WidgetRegister)]
pub struct FloatingAction {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[live]
    draw_shadow: DrawFloatingShadow,

    #[live]
    pub anchor: FloatingAnchor,
    #[live]
    pub pin_to: FloatingPin,
    /// Laid out in the parent's flow; `pin_to` and `edge_margin` are not
    /// read.
    #[live(false)]
    pub inline: bool,
    #[live]
    pub edge_margin: Inset,
    /// Named `dial` because `layout` is the widget's own layout.
    #[live]
    pub dial: SpeedDialLayout,
    #[live]
    pub labels: SpeedDialLabels,
    #[live(56.0)]
    pub size: f64,
    #[live(40.0)]
    pub item_size: f64,
    #[live(12.0)]
    pub item_gap: f64,
    #[live(88.0)]
    pub radial_radius: f64,
    /// The furthest an arc is laid from the button's middle; 0 is no limit.
    /// Left to the window alone a large set grows one arc as wide as the
    /// window lets it be, and eight from a corner rest four and a half
    /// buttons away from the hand that opened them. Held inside this radius
    /// the set goes on round further arcs a step apart, the inner filled
    /// first, as it does in a window that small. The default holds up to
    /// twelve from a corner. A set no arcs inside it can hold is neither
    /// shrunk nor cut: it rests on the arcs of the least room past this
    /// radius that holds it.
    #[live(200.0)]
    pub radial_max_radius: f64,
    #[live(true)]
    pub close_on_pick: bool,
    /// Out from the first draw and kept out: no pointer grab, no outside
    /// or Escape close, picks still reported. For catalogues and teaching
    /// overlays, as the pie menu's `pinned` is.
    #[live(false)]
    pub pinned: bool,
    #[live(0.2)]
    pub enter_secs: f64,
    /// How the actions travel out and the plus turns to the cross.
    #[live(Ease::OutCubic)]
    pub enter_ease: Ease,
    #[live(0.15)]
    pub exit_secs: f64,
    /// How the actions travel back and the cross turns back to the plus.
    #[live(Ease::InCubic)]
    pub exit_ease: Ease,
    #[live(0.03)]
    pub stagger_secs: f64,
    #[live(false)]
    pub reduced_motion: bool,
    #[live(true)]
    visible: bool,

    #[rust]
    children: Vec<(LiveId, WidgetRef)>,
    #[rust]
    update_order: Vec<LiveId>,
    /// The `main :=` child.
    #[rust]
    main: WidgetRef,
    /// Every `FloatingActionItem` child, in declaration order.
    #[rust]
    items: Vec<(LiveId, WidgetRef)>,
    /// Children of another type, already reported once.
    #[rust]
    ignored: Vec<LiveId>,
    #[rust]
    draw_list: Option<DrawList2d>,

    #[rust]
    open: bool,
    /// When the legs below set off.
    #[rust]
    motion_started: f64,
    /// Each shown action's leg, in layout order.
    #[rust]
    legs: Vec<Leg>,
    /// The glyph's leg: 0 the plus, 1 the cross.
    #[rust]
    turn: Leg,
    /// The glyph value last written to the main face.
    #[rust]
    turn_written: f64,
    /// Where the shown actions rest, laid out by the last draw around
    /// `plan_centre`. Presses and arrow keys read this, never where an
    /// action happens to be mid-travel.
    #[rust]
    plan: DialPlan,
    #[rust]
    plan_centre: DVec2,
    /// An action pressed while the set was still moving. Its button did not
    /// hear the press, so the set picks it when the release comes over the
    /// same action.
    #[rust]
    pressed_item: Option<usize>,
    /// From the press that opened the set until its release: a release on
    /// an action picks it, so press, drag and release is one gesture.
    #[rust]
    opening_press: bool,
    /// The sweep lock is owed but not taken: the set opened on a press, and
    /// the main button must still see the release of that press or it
    /// stays drawn as held.
    #[rust]
    lock_pending: bool,
    #[rust]
    locked: bool,
    /// The area the lock was last taken with.
    #[rust]
    lock_area: Area,
    /// A press outside closed the set; its release is eaten before the lock
    /// goes, so the dismissing click does not act on what was underneath.
    #[rust]
    swallow_up: bool,
    /// The action under the pointer, for Hot chips.
    #[rust]
    hot_item: Option<usize>,
    /// Opened from the keyboard: key focus moves to the first action once
    /// the actions have areas to take it with.
    #[rust]
    focus_first: bool,
    /// This pass opened the set from a key, so the button's own keyboard
    /// activation must not be read as a second press.
    ///
    /// A Button activates from Return and Space on its own account, and this
    /// widget answers the same keys itself to put focus on the first action.
    /// Without this the two agree: the key opens the set here, the button's
    /// `Pressed` arrives a moment later, `handle_actions` finds the set open
    /// and closes it again -- one keypress, open and shut, with the focus the
    /// open was for thrown away.
    #[rust]
    opened_by_key: bool,
    /// The trap was begun before the set was drawn and must be pointed at
    /// the draw that has the stops in it.
    #[rust]
    trap_pending: bool,
    #[rust]
    focus_trap: FocusTrap,

    /// The container's rect, recorded in its own list so the event side can
    /// read where alignment really put it.
    #[rust]
    host_area: Area,
    #[rust]
    host_honest: Rect,
    /// The container's rect as seen mid-draw, the last draw.
    #[rust]
    host_seen: Rect,
    /// The container rect the last draw pinned against.
    #[rust]
    host_used: Rect,
    /// The part of the container its own ancestors leave on screen, when
    /// that is not all of it: a page scrolled until the container is half
    /// under its top edge. The overlay is cut to it, or a button pinned to
    /// a container that has scrolled away would float on over whatever the
    /// page scrolled under.
    #[rust]
    host_visible: Option<Rect>,
    /// The main button's rect, window-absolute and after alignment.
    #[rust]
    main_honest: Rect,
    /// Inline: the main button's rect as seen mid-draw, the last draw.
    #[rust]
    main_seen: Rect,
    /// The centre the set was last laid out around.
    #[rust]
    centre_used: DVec2,
    #[rust]
    next_frame: NextFrame,
    /// One frame after a draw, to compare what was drawn with where
    /// alignment put it.
    #[rust]
    check_frame: NextFrame,
    #[rust]
    was_pinned: bool,
}

impl ScriptHook for FloatingAction {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }

    fn on_before_apply(&mut self, _vm: &mut ScriptVm, apply: &Apply, _scope: &mut Scope, _value: ScriptValue) {
        if !apply.is_eval() {
            self.update_order.clear();
        }
    }

    fn on_after_apply(&mut self, vm: &mut ScriptVm, apply: &Apply, scope: &mut Scope, value: ScriptValue) {
        apply_children(vm, apply, scope, value, &mut self.children, &mut self.update_order);
        self.sort_children();
        let uid = self.uid;
        vm.with_cx_mut(|cx| {
            cx.widget_tree_mark_dirty(uid);
            // Unpinned while out: the set was never given a lock or a trap,
            // so it is put away rather than left out with neither.
            let unpinned = self.was_pinned && !self.pinned;
            if self.open && (unpinned || self.shown_items().is_empty()) {
                self.shut(cx, false);
            }
            // Pinned while out: a pinned set never closes, so a grab and a
            // trap taken for the open would never be given back, and every
            // press outside the set would go on being turned away.
            if self.open && self.pinned && !self.was_pinned {
                self.opening_press = false;
                self.lock_pending = false;
                self.swallow_up = false;
                self.unlock(cx);
                self.trap_pending = false;
                self.focus_trap.end(cx);
            }
            self.was_pinned = self.pinned;
            self.redraw(cx);
        });
    }
}

impl WidgetNode for FloatingAction {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    /// Pinned, it claims no room: the button floats over the container and
    /// a walk of any size would push the container's content aside, or,
    /// as a fill, take a share of it. Inline, it is the button's own size.
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        if self.inline {
            Walk {
                width: Size::Fixed(self.size),
                height: Size::Fixed(self.size),
                ..self.walk
            }
        } else {
            Walk::empty()
        }
    }

    /// The main button's area, so the test tree reports the button's rect
    /// for the widget.
    fn area(&self) -> Area {
        self.main.area()
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.main.redraw(cx);
        if let Some(draw_list) = &self.draw_list {
            draw_list.redraw(cx);
        }
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        if !self.main.is_empty() {
            visit(live_id!(main), self.main.clone());
        }
        for (id, item) in &self.items {
            visit(*id, item.clone());
        }
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        self.main.find_widgets_from_point(cx, point, found);
        if self.open {
            for (_, item) in &self.items {
                item.find_widgets_from_point(cx, point, found);
            }
        }
    }

    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if !visible {
            self.shut(cx, false);
            self.unlock(cx);
            self.swallow_up = false;
            // Nothing is drawn to watch the set go away, so it is away now.
            self.land(0.0);
        }
        if visible && self.main.area().is_empty() {
            // Never drawn, so nothing knows where to redraw it from.
            cx.redraw_all();
        } else {
            self.host_area.redraw(cx);
            self.redraw(cx);
        }
    }

    fn visible(&self) -> bool {
        self.visible
    }
}

impl FloatingAction {
    fn sort_children(&mut self) {
        self.main = WidgetRef::default();
        let mut items = Vec::new();
        let owner = self.draw_list.as_ref().map(|list| list.id());
        for (id, child) in &self.children {
            if *id == live_id!(main) {
                self.main = child.clone();
            } else if let Some(mut inner) = child.borrow_mut::<FloatingActionItem>() {
                inner.owner_list = owner;
                items.push((*id, child.clone()));
            } else if !self.ignored.contains(id) {
                self.ignored.push(*id);
                log!("FloatingAction: child {} is not a FloatingActionItem and is ignored", id);
            }
        }
        if items.len() != self.items.len() {
            self.hot_item = None;
            self.pressed_item = None;
        }
        self.items = items;
    }

    fn item_visible(&self, i: usize) -> bool {
        self.items
            .get(i)
            .and_then(|(_, item)| item.borrow::<FloatingActionItem>().map(|inner| inner.visible))
            .unwrap_or(false)
    }

    /// The actions in the set, as indices into `items`, in layout order.
    fn shown_items(&self) -> Vec<usize> {
        (0..self.items.len()).filter(|i| self.item_visible(*i)).collect()
    }

    fn ease_toward(&self, to: f64) -> Ease {
        if to > 0.5 {
            self.enter_ease
        } else {
            self.exit_ease
        }
    }

    /// Every shown action's position `now`. The legs are matched to the
    /// shown set first: an action shown or hidden while the set is out joins
    /// it where the set is headed.
    fn positions(&mut self, now: f64) -> Vec<f64> {
        let n = self.shown_items().len();
        if self.legs.len() != n {
            let rest = Leg::at_rest(if self.open { 1.0 } else { 0.0 });
            self.legs.resize(n, rest);
        }
        let elapsed = now - self.motion_started;
        let (enter, exit) = (self.enter_ease, self.exit_ease);
        self.legs
            .iter()
            .map(|leg| leg.value(elapsed, if leg.to > 0.5 { &enter } else { &exit }))
            .collect()
    }

    fn turn_value(&self, now: f64) -> f64 {
        self.turn.value(now - self.motion_started, &self.ease_toward(self.turn.to))
    }

    /// Whether any action is still on its way.
    fn travelling(&self, now: f64) -> bool {
        let elapsed = now - self.motion_started;
        self.legs.iter().any(|leg| !leg.done(elapsed))
    }

    /// Whether anything, an action or the glyph, is still on its way.
    fn moving(&self, now: f64) -> bool {
        self.travelling(now) || !self.turn.done(now - self.motion_started)
    }

    /// Set every action and the glyph off toward `to`, from wherever each is.
    /// Called before `open` changes, so an action that joined the set since
    /// the last draw starts from where the set was.
    fn set_off(&mut self, cx: &mut Cx, to: f64) {
        let now = cx.seconds_since_app_start();
        let positions = self.positions(now);
        let turn = self.turn_value(now);
        if self.reduced_motion {
            self.land(to);
        } else {
            let secs = if to > 0.5 { self.enter_secs } else { self.exit_secs };
            let step = stagger_step(positions.len(), self.stagger_secs);
            self.legs = legs_to(&positions, to, secs, step);
            self.turn = legs_to(&[turn], to, secs, 0.0)[0];
        }
        self.motion_started = now;
        self.next_frame = cx.new_next_frame();
    }

    /// Everything already at `to`.
    fn land(&mut self, to: f64) {
        let n = self.shown_items().len();
        self.legs = vec![Leg::at_rest(to); n];
        self.turn = Leg::at_rest(to);
    }

    /// Write the glyph's turn into the main face.
    ///
    /// The button's own open track is cut to its end the moment the set
    /// opens or closes, and the set plays the turn instead, on its own eases
    /// and clock, so the plus turns with the actions rather than on a curve
    /// of its own. It is written the way an animation frame is, which
    /// updates the face without rebuilding the button, and on every draw
    /// while it turns: the button's other tracks re-apply the cut value on
    /// their own frames.
    fn write_turn(&mut self, cx: &mut Cx, now: f64) {
        let value = self.turn_value(now);
        let turning = !self.turn.done(now - self.motion_started);
        if !turning && (value - self.turn_written).abs() < 1e-9 {
            return;
        }
        let Some(mut button) = self.main.borrow_mut::<Button>() else {
            return;
        };
        self.turn_written = value;
        cx.with_vm(|vm| {
            let face = vm.bx.heap.new_object();
            vm.bx.heap.set_value_def(face, live_id!(open).into(), value.into());
            let apply = vm.bx.heap.new_object();
            vm.bx.heap.set_value_def(apply, live_id!(draw_bg).into(), face.into());
            button.script_apply(vm, &Apply::Animate, &mut Scope::empty(), apply.into());
        });
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Bring the set out. Nothing happens without a set: a floating action
    /// with no actions behind it is just the action.
    pub fn open(&mut self, cx: &mut Cx) {
        self.open_with(cx, false);
    }

    /// Put the set away. A pinned set stays: there is nothing to dismiss.
    pub fn close(&mut self, cx: &mut Cx) {
        if !self.pinned {
            self.shut(cx, false);
        }
    }

    pub fn toggle(&mut self, cx: &mut Cx) {
        if self.open {
            self.close(cx);
        } else {
            self.open(cx);
        }
    }

    pub fn set_anchor(&mut self, cx: &mut Cx, anchor: FloatingAnchor) {
        if self.anchor != anchor {
            self.anchor = anchor;
            self.redraw(cx);
        }
    }

    pub fn set_dial(&mut self, cx: &mut Cx, dial: SpeedDialLayout) {
        if self.dial != dial {
            self.dial = dial;
            self.hot_item = None;
            self.redraw(cx);
        }
    }

    pub fn set_item_enabled(&mut self, cx: &mut Cx, id: LiveId, enabled: bool) {
        if let Some((_, item)) = self.items.iter().find(|(item_id, _)| *item_id == id) {
            if let Some(mut inner) = item.borrow_mut::<FloatingActionItem>() {
                inner.set_enabled(cx, enabled);
            }
        }
    }

    /// The main button's rect, window-absolute and after alignment.
    pub fn main_rect(&self) -> Rect {
        self.main_honest
    }

    fn open_with(&mut self, cx: &mut Cx, from_press: bool) {
        if self.open || self.shown_items().is_empty() {
            return;
        }
        self.set_off(cx, 1.0);
        self.open = true;
        self.hot_item = None;
        self.pressed_item = None;
        self.swallow_up = false;
        if !self.pinned {
            self.opening_press = from_press;
            if self.locked {
                // Still held from a dismissing press whose release has not
                // come: keep it, it is the same grab.
            } else if from_press {
                self.lock_pending = true;
            } else {
                self.lock(cx);
            }
            self.focus_trap.begin(cx, self.main.area());
            self.trap_pending = true;
        }
        // The turn is played by the set; the button's track only has to end
        // where the set is going.
        self.main.as_button().set_open_with(cx, true, Animate::No);
        self.redraw(cx);
        cx.widget_action(self.uid, FloatingActionAction::Opened);
    }

    /// Put the set away, pinned or not. `keep_lock` holds the grab until the
    /// release of the press that caused it.
    fn shut(&mut self, cx: &mut Cx, keep_lock: bool) {
        if !self.open {
            return;
        }
        self.set_off(cx, 0.0);
        self.open = false;
        self.opening_press = false;
        self.lock_pending = false;
        self.hot_item = None;
        self.pressed_item = None;
        for (_, item) in &self.items {
            if let Some(mut inner) = item.borrow_mut::<FloatingActionItem>() {
                inner.chip_rect = None;
            }
        }
        self.focus_first = false;
        self.opened_by_key = false;
        self.trap_pending = false;
        if keep_lock && self.locked {
            self.swallow_up = true;
        } else {
            self.swallow_up = false;
            self.unlock(cx);
        }
        self.focus_trap.end(cx);
        self.main.as_button().set_open_with(cx, false, Animate::No);
        self.redraw(cx);
        cx.widget_action(self.uid, FloatingActionAction::Closed);
    }

    fn lock(&mut self, cx: &mut Cx) {
        let area = self.main.area();
        cx.sweep_lock(area);
        self.lock_area = area;
        self.locked = true;
    }

    fn unlock(&mut self, cx: &mut Cx) {
        if self.locked {
            // Both: a redraw moves the lock to the button's newer area, and
            // an order change between a closed and an open draw can give
            // that area another slot than the one it was taken with.
            cx.sweep_unlock(self.lock_area);
            cx.sweep_unlock(self.main.area());
            self.locked = false;
        }
    }

    fn pick(&mut self, cx: &mut Cx, id: LiveId) {
        cx.widget_action(self.uid, FloatingActionAction::Picked(id));
        if self.close_on_pick {
            self.close(cx);
        }
    }

    fn item_enabled(&self, i: usize) -> bool {
        self.items
            .get(i)
            .and_then(|(_, item)| item.borrow::<FloatingActionItem>().map(|inner| inner.enabled))
            .unwrap_or(false)
    }

    fn item_button_area(&self, i: usize) -> Area {
        self.items
            .get(i)
            .and_then(|(_, item)| item.borrow::<FloatingActionItem>().map(|inner| inner.button.area()))
            .unwrap_or_default()
    }

    /// Every area a press on this set can belong to: the button, and each
    /// action's face. Asked of [`CxFingers::is_mouse_held_outside`], so
    /// that the set's OWN press is never mistaken for somebody else's —
    /// which matters here more than anywhere, because the set's one gesture
    /// is a press on the button, a drag down the dial and a release on an
    /// action, held by the BUTTON from beginning to end.
    fn own_areas(&self) -> Vec<Area> {
        let mut areas = vec![self.main.area(), self.lock_area];
        areas.extend((0..self.items.len()).map(|i| self.item_button_area(i)));
        areas
    }

    /// Whether the mouse is held by a control this set does not own.
    ///
    /// The set reads presses and moves from RAW events — it has to, because
    /// while the actions are travelling their buttons hit-test places they
    /// have not reached yet — and a raw reader is told nothing about who
    /// holds the pointer. So it asks. A slider, a scroll bar, a resizer
    /// mid-drag holds the mouse until its release, and until then a press
    /// or a hover read here would be a second answer to one press.
    ///
    /// Only the mouse locks: a touch capture answers false, and the touch
    /// paths below are left as they are.
    fn mouse_held_elsewhere(&self, cx: &Cx) -> bool {
        cx.fingers.is_mouse_held_outside(&self.own_areas())
    }

    /// The shown action resting under `abs`: where the set laid it out, not
    /// where its travel has it this frame. The layout keeps resting squares
    /// apart; in a window too small for the set, where they can meet, the
    /// nearer middle wins.
    fn item_at(&self, abs: DVec2) -> Option<usize> {
        if !self.open {
            return None;
        }
        let half = self.item_size * 0.5;
        self.shown_items()
            .into_iter()
            .zip(self.plan.offsets.iter())
            .filter_map(|(i, offset)| {
                let d = abs - (self.plan_centre + *offset);
                (d.x.abs() <= half && d.y.abs() <= half).then_some((i, d.length()))
            })
            .fold(None::<(usize, f64)>, |best, c| match best {
                Some(b) if b.1 <= c.1 => Some(b),
                _ => Some(c),
            })
            .map(|c| c.0)
    }

    /// Whether `abs` is on anything the set drew: the button, an action or
    /// a chip. A press anywhere else is a press outside.
    fn on_dial(&self, cx: &Cx, abs: DVec2) -> bool {
        let main = self.main.area();
        if !main.is_empty() && has_size(main.rect(cx)) && main.rect(cx).contains(abs) {
            return true;
        }
        if self.item_at(abs).is_some() {
            return true;
        }
        self.items.iter().any(|(_, item)| {
            item.borrow::<FloatingActionItem>()
                .and_then(|inner| inner.chip_rect)
                .is_some_and(|rect| rect.contains(abs))
        })
    }

    fn focused_item(&self, cx: &Cx) -> Option<usize> {
        (0..self.items.len()).find(|i| {
            let area = self.item_button_area(*i);
            !area.is_empty() && cx.has_key_focus(area)
        })
    }

    fn focus_item(&mut self, cx: &mut Cx, i: usize) {
        let area = self.item_button_area(i);
        if !area.is_empty() {
            cx.set_key_focus(area);
        }
    }

    /// The shown actions that answer presses, in layout order.
    fn enabled_items(&self) -> Vec<usize> {
        self.shown_items().into_iter().filter(|i| self.item_enabled(*i)).collect()
    }

    /// Move key focus the way an arrow points, among the enabled actions'
    /// resting places. An arc is walked in order, inner arc first; a row or
    /// a column, folded or not, by direction, where a folded line's
    /// neighbour across is the one the arrow means.
    fn step_focus(&mut self, cx: &mut Cx, focus: Option<usize>, dir: DVec2) {
        let shown = self.shown_items();
        let mut enabled = Vec::new();
        let mut points = Vec::new();
        let mut first_arc = 0;
        for (k, i) in shown.iter().enumerate() {
            let (Some(offset), Some(row)) = (self.plan.offsets.get(k), self.plan.rows.get(k)) else {
                continue;
            };
            if self.item_enabled(*i) {
                enabled.push(*i);
                points.push(*offset);
                if *row == 0 {
                    first_arc += 1;
                }
            }
        }
        let from = focus.and_then(|f| enabled.iter().position(|i| *i == f));
        let origin = dvec2(0.0, 0.0);
        let target = if self.dial == SpeedDialLayout::Radial {
            arc_walk_target(&points, first_arc, from, origin, dir)
        } else {
            arrow_target(&points, from, origin, dir)
        };
        if let Some(k) = target {
            self.focus_item(cx, enabled[k]);
        }
    }

    /// Read where alignment really put the host and, inline, the button,
    /// and redraw when the set was laid out against somewhere else.
    fn refresh_honest(&mut self, cx: &mut Cx) {
        let mut moved = false;
        if self.inline {
            let area = self.main.area();
            if !area.is_empty() {
                let rect = area.rect(cx);
                if has_size(rect) {
                    // Closed as well as open: the shadow is laid out around
                    // the same centre as the actions.
                    let centre = rect.center();
                    if (centre.x - self.centre_used.x).abs() > HONEST_SLACK
                        || (centre.y - self.centre_used.y).abs() > HONEST_SLACK
                    {
                        moved = true;
                    }
                    self.main_honest = rect;
                }
            }
        } else if self.pin_to == FloatingPin::Container && !self.host_area.is_empty() {
            let rect = self.host_area.rect(cx);
            if has_size(rect) {
                if !near(rect, self.host_used) {
                    moved = true;
                }
                self.host_honest = rect;
                let clipped = self.host_area.clipped_rect(cx);
                let finite = clipped.pos.x.is_finite()
                    && clipped.pos.y.is_finite()
                    && clipped.size.x.is_finite()
                    && clipped.size.y.is_finite();
                let visible = (finite && !near(clipped, rect)).then_some(clipped);
                let same = match (visible, self.host_visible) {
                    (Some(a), Some(b)) => near(a, b),
                    (None, None) => true,
                    _ => false,
                };
                if !same {
                    moved = true;
                }
                self.host_visible = visible;
            }
        }
        if moved {
            self.redraw(cx);
        }
    }

    fn release_at(&mut self, cx: &mut Cx, abs: DVec2) {
        if !self.opening_press {
            return;
        }
        self.opening_press = false;
        if self.open && self.lock_pending {
            self.lock_pending = false;
            self.lock(cx);
        }
        if let Some(i) = self.item_at(abs) {
            if self.item_enabled(i) {
                let id = self.items[i].0;
                self.pick(cx, id);
            }
        }
    }

    /// A press while the actions are still moving, on the resting place of
    /// one of them. Their buttons are not hearing the pointer, so the set
    /// holds the press itself. Answers whether it did.
    fn press_moving_item(&mut self, abs: DVec2) -> bool {
        match self.item_at(abs) {
            Some(i) => {
                self.pressed_item = Some(i);
                true
            }
            None => false,
        }
    }

    /// The release of a press `press_moving_item` held: a pick when it comes
    /// over the same action, as a click on its button would be.
    fn release_moving_item(&mut self, cx: &mut Cx, abs: DVec2) {
        if let Some(i) = self.pressed_item.take() {
            if self.item_at(abs) == Some(i) && self.item_enabled(i) {
                let id = self.items[i].0;
                self.pick(cx, id);
            }
        }
    }

    /// A press from a mouse button or the first touch. Answers whether the
    /// press was taken.
    fn press_at(&mut self, cx: &mut Cx, abs: DVec2, on_top: bool) -> bool {
        self.refresh_honest(cx);
        if self.open && !self.pinned && on_top && !self.on_dial(cx, abs) {
            self.shut(cx, true);
            return true;
        }
        false
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let main = self.main.as_button();
        if self.shown_items().is_empty() {
            if main.clicked(actions) {
                cx.widget_action(self.uid, FloatingActionAction::Pressed);
            }
        } else if main.pressed(actions) && self.opened_by_key {
            // The key that opened it reaching us a second time, through the
            // button. Swallow it once; the set stays open.
            self.opened_by_key = false;
        } else if main.pressed(actions) && !self.pinned {
            // On the press, not the click: the set is out while the hand is
            // still down, so dragging to an action and letting go picks it.
            // The click that follows is the same gesture and is not read.
            if self.open {
                self.close(cx);
            } else {
                self.open_with(cx, true);
            }
        }
        let picked = self.items.iter().find_map(|(id, item)| {
            let button = item.borrow::<FloatingActionItem>().map(|inner| inner.button.clone())?;
            button.as_button().clicked(actions).then_some(*id)
        });
        if let Some(id) = picked {
            self.pick(cx, id);
        }
    }

    fn key_down(&mut self, cx: &mut Cx, event: &Event, ke: &KeyEvent, on_top: bool) {
        let activate = matches!(ke.key_code, KeyCode::ReturnKey | KeyCode::Space);
        let main_area = self.main.area();
        let main_focused = !main_area.is_empty() && cx.has_key_focus(main_area);
        if !self.open {
            if activate && main_focused {
                if self.shown_items().is_empty() {
                    cx.widget_action(self.uid, FloatingActionAction::Pressed);
                } else if !self.pinned {
                    self.open_with(cx, false);
                    self.focus_first = true;
                    self.opened_by_key = true;
                }
            }
            return;
        }
        if ke.key_code == KeyCode::Escape {
            if !self.pinned && on_top && claim_escape(cx) {
                self.close(cx);
                cx.set_key_focus(self.main.area());
            }
            return;
        }
        if ke.key_code == KeyCode::Tab && self.focus_trap.is_active() {
            self.tab(cx, ke.modifiers.shift);
            return;
        }
        if self.focus_trap.handle_event(cx, event) {
            return;
        }
        let focus = self.focused_item(cx);
        // The keyboard is somewhere else on the page: a pinned set must not
        // take the arrows from a text field.
        if focus.is_none() && !main_focused {
            return;
        }
        match ke.key_code {
            KeyCode::ArrowUp => self.step_focus(cx, focus, dvec2(0.0, -1.0)),
            KeyCode::ArrowDown => self.step_focus(cx, focus, dvec2(0.0, 1.0)),
            KeyCode::ArrowLeft => self.step_focus(cx, focus, dvec2(-1.0, 0.0)),
            KeyCode::ArrowRight => self.step_focus(cx, focus, dvec2(1.0, 0.0)),
            KeyCode::Home => {
                if let Some(first) = self.enabled_items().first().copied() {
                    self.focus_item(cx, first);
                }
            }
            KeyCode::End => {
                if let Some(last) = self.enabled_items().last().copied() {
                    self.focus_item(cx, last);
                }
            }
            _ if activate => match focus {
                Some(i) if self.item_enabled(i) => {
                    let id = self.items[i].0;
                    self.pick(cx, id);
                    if !self.open {
                        cx.set_key_focus(self.main.area());
                    }
                }
                Some(_) => {}
                None => self.close(cx),
            },
            _ => {}
        }
    }

    /// The trap's Tab, without the disabled actions. The trap walks every
    /// stop under the button's list, and a disabled action's button
    /// registers one like any other, so Tab would rest on an action that
    /// Return cannot pick.
    fn tab(&mut self, cx: &mut Cx, back: bool) {
        let disabled: Vec<Area> = (0..self.items.len())
            .filter(|i| !self.item_enabled(*i) || !self.item_visible(*i))
            .map(|i| self.item_button_area(i))
            .collect();
        let stops: Vec<Area> = self
            .focus_trap
            .stops(cx)
            .into_iter()
            .filter(|stop| !disabled.contains(stop))
            .collect();
        if let Some(next) = tab_target(&stops, cx.key_focus(), back) {
            cx.set_key_focus(next);
        }
    }

    fn draw_main_shadow(&mut self, cx: &mut Cx2d, face: Rect) {
        let reach = shadow_reach(&self.draw_shadow, cx);
        self.draw_shadow.radius = (face.size.x.min(face.size.y) * 0.5) as f32;
        self.draw_shadow.weight = 1.0;
        self.draw_shadow.draw_abs(cx, grown(face, reach));
    }

    /// Draw every shown action around `centre`, each where its travel has
    /// got to, and lay out where they rest for the event side.
    fn draw_items(&mut self, cx: &mut Cx2d, scope: &mut Scope, centre: DVec2, window: Rect) {
        let now = cx.seconds_since_app_start();
        let shown = self.shown_items();
        let plan = dial_plan(
            self.anchor,
            self.dial,
            shown.len(),
            self.size,
            self.item_size,
            self.item_gap,
            self.radial_radius,
            self.radial_max_radius,
            centre,
            window,
        );
        let positions = self.positions(now);
        let elapsed = now - self.motion_started;
        let inward = interior(self.anchor);
        let labels = labels_shown_in(self.labels, self.dial, plan.row_count());
        let mut faces: Vec<(DVec2, f64)> = plan.offsets.iter().map(|offset| (centre + *offset, self.item_size)).collect();
        faces.push((centre, self.size));
        for (k, &i) in shown.iter().enumerate() {
            let v = positions[k];
            let offset = plan.offsets[k];
            let focused = {
                let area = self.item_button_area(i);
                !area.is_empty() && cx.has_key_focus(area)
            };
            // Words only once the action has arrived, and not on its way
            // back: a chip beside a disc still travelling hangs in the air.
            let arrived = self.open && self.legs[k].done(elapsed);
            let show_chip = arrived
                && match labels {
                    SpeedDialLabels::Always => true,
                    SpeedDialLabels::Hot => self.hot_item == Some(i) || focused,
                    SpeedDialLabels::Never | SpeedDialLabels::Auto => false,
                };
            let size = self.item_size * (START_SCALE + (1.0 - START_SCALE) * v.clamp(0.0, 1.0));
            let rest = Rect {
                pos: centre + offset - self.item_size * 0.5,
                size: dvec2(self.item_size, self.item_size),
            };
            let (side, beyond) = chip_place(self.anchor, self.dial, plan.rows[k], &plan, self.item_size, self.item_gap);
            let item = self.items[i].1.clone();
            if let Some(mut inner) = item.borrow_mut::<FloatingActionItem>() {
                inner.place = ItemPlace {
                    centre: keep_inside(centre + offset * v, size, bounding(window, rest)),
                    size,
                    side,
                    outward: offset,
                    beyond,
                    inward,
                    window,
                    show_chip,
                };
                inner.draw_face(cx, scope);
            };
        }
        for &i in &shown {
            if let Some(mut inner) = self.items[i].1.borrow_mut::<FloatingActionItem>() {
                inner.draw_label_chip(cx, &faces);
            }
        }
        for (i, (_, item)) in self.items.iter().enumerate() {
            if !shown.contains(&i) {
                if let Some(mut inner) = item.borrow_mut::<FloatingActionItem>() {
                    inner.chip_rect = None;
                }
            }
        }
        self.plan = plan;
        self.plan_centre = centre;
        if self.moving(now) {
            self.next_frame = cx.new_next_frame();
        }
    }
}

impl Drop for FloatingAction {
    fn drop(&mut self) {
        if self.locked {
            orphan_sweep_locks(&[self.lock_area, self.main.area()]);
        }
    }
}

impl Widget for FloatingAction {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // Before anything can take a lock of its own this event. The window
        // does this too; this is for a tree with no window above it.
        release_orphaned_sweep_locks(cx);
        // The clock is read only by the events that need it.
        if self.next_frame.is_event(event).is_some() && (self.open || self.moving(cx.seconds_since_app_start())) {
            self.redraw(cx);
        }
        if self.check_frame.is_event(event).is_some() {
            self.refresh_honest(cx);
        }
        // A set cannot outlive its window's focus, and a release a
        // dismissing press was waiting for is never coming.
        if matches!(event, Event::WindowLostFocus(_) | Event::Pause | Event::Background) {
            if !self.pinned {
                self.shut(cx, false);
            }
            self.swallow_up = false;
            self.pressed_item = None;
            self.unlock(cx);
        }
        if !self.visible {
            return;
        }

        // Whether this widget's lock is the innermost, read before it is
        // lifted: an overlay opened above it gets Escape and the outside
        // press first.
        let on_top = !self.locked || {
            let top = cx.sweep_lock_area();
            top == Some(self.lock_area) || top == Some(self.main.area())
        };

        // Lifted only while it is the innermost. Taken again, a lock that
        // was not on top would go back on top, above the overlay that had
        // opened over this set, and that overlay's hits would go dead; and
        // under another lock the children's plain hits are turned away
        // whether this one is lifted or not.
        let held = self.locked && on_top;
        if held {
            cx.sweep_unlock(self.lock_area);
            cx.sweep_unlock(self.main.area());
        }
        // While the actions are still moving, the hand aims at where they are
        // going, not where they happen to be this frame. Their buttons would
        // hit-test the frame's place, so they do not hear the pointer until
        // they arrive, and the set reads presses on the resting places.
        let pointer = matches!(
            event,
            Event::MouseDown(_) | Event::MouseMove(_) | Event::MouseUp(_) | Event::MouseLeave(_) | Event::TouchUpdate(_) | Event::LongPress(_)
        );
        let shielded = pointer && (self.pressed_item.is_some() || self.travelling(cx.seconds_since_app_start()));
        self.main.handle_event(cx, event, scope);
        if !shielded {
            for (_, item) in &self.items {
                item.handle_event(cx, event, scope);
            }
        }
        if held {
            let area = self.main.area();
            cx.sweep_lock(area);
            self.lock_area = area;
        }

        match event {
            Event::Actions(actions) => self.handle_actions(cx, actions),
            Event::MouseDown(me) => {
                // Holding a travelling action is a press-like state taken
                // from a raw press, so it asks first. The dismissal below
                // does not: putting a set away because the person pressed
                // somewhere else is not a gesture competing for the pointer,
                // and a set left out over a page being worked is the bug it
                // was written to fix.
                let taken = (shielded
                    && self.open
                    && !self.mouse_held_elsewhere(cx)
                    && self.press_moving_item(me.abs))
                    || self.press_at(cx, me.abs, on_top);
                if taken && me.handled.get().is_empty() {
                    me.handled.set(self.main.area());
                }
            }
            Event::MouseMove(me) => {
                if self.open {
                    // An action lit under a pointer somebody else is holding
                    // offers a press that cannot arrive: that mouse is going
                    // back to the control that took it, whatever it passes
                    // over on the way. The set's own press is not elsewhere,
                    // so the hand still lights each action it travels over
                    // on its way down the dial.
                    let hot = if self.mouse_held_elsewhere(cx) {
                        None
                    } else {
                        self.item_at(me.abs)
                    };
                    if hot != self.hot_item {
                        self.hot_item = hot;
                        if labels_shown_in(self.labels, self.dial, self.plan.row_count()) == SpeedDialLabels::Hot {
                            self.redraw(cx);
                        }
                    }
                }
            }
            Event::MouseUp(me) => {
                if self.swallow_up {
                    self.swallow_up = false;
                    if !self.open {
                        self.unlock(cx);
                    }
                }
                self.release_moving_item(cx, me.abs);
                self.release_at(cx, me.abs);
            }
            // The mouse press itself taken away: its bookkeeping ends as a
            // release's would, but nothing is picked.
            Event::FingerCancel(c) if c.device.is_mouse() && cx.fingers.press_taken_away(c.digit_id) => {
                if self.swallow_up {
                    self.swallow_up = false;
                    if !self.open {
                        self.unlock(cx);
                    }
                }
                self.pressed_item = None;
                if std::mem::take(&mut self.opening_press) && self.open && self.lock_pending {
                    self.lock_pending = false;
                    self.lock(cx);
                }
            }
            // Touch never becomes a mouse press: without this arm a set
            // opens on a phone and a tap outside never puts it away. The
            // first touch only.
            Event::TouchUpdate(te) => {
                if let Some(touch) = te.touches.first() {
                    match touch.state {
                        TouchState::Start => {
                            let taken = (shielded && self.open && self.press_moving_item(touch.abs))
                                || self.press_at(cx, touch.abs, on_top);
                            if taken && touch.handled.get().is_empty() {
                                touch.handled.set(self.main.area());
                            }
                        }
                        TouchState::Stop => {
                            if self.swallow_up {
                                self.swallow_up = false;
                                if !self.open {
                                    self.unlock(cx);
                                }
                            }
                            self.release_moving_item(cx, touch.abs);
                            self.release_at(cx, touch.abs);
                        }
                        _ => {}
                    }
                }
            }
            Event::KeyDown(ke) => self.key_down(cx, event, ke, on_top),
            _ => {}
        }
        if self.open && !self.pinned && on_top && event.back_pressed() {
            self.close(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // The overlay list is begun on every draw, shown or not: a list that
        // is not begun keeps showing what it showed last. The handle is taken
        // out of `self` so the borrow does not pin every other field.
        let Some(mut draw_list) = self.draw_list.take() else {
            return DrawStep::done();
        };
        let owner = Some(draw_list.id());
        for (_, item) in &self.items {
            if let Some(mut inner) = item.borrow_mut::<FloatingActionItem>() {
                inner.owner_list = owner;
            }
        }
        // Only a parent that draws its children whatever their flag gets
        // here. A view skips a hidden child altogether, and the list then
        // goes on the next draw of the list it was begun in: the window's
        // overlay drops a list its parent was redrawn without.
        if !self.visible {
            draw_list.begin_overlay_reuse(cx);
            cx.begin_root_turtle_for_pass(Layout::default());
            cx.end_pass_sized_turtle();
            draw_list.end(cx);
            self.draw_list = Some(draw_list);
            return DrawStep::done();
        }

        if self.pinned && !self.open && !self.shown_items().is_empty() {
            // Out from the first draw, already turned: a pinned set was never
            // closed, so there is nothing to watch it open from.
            self.open = true;
            self.land(1.0);
            self.main.as_button().set_open_with(cx, true, Animate::No);
        }
        let now = cx.seconds_since_app_start();
        let dial_drawn = (self.open || self.moving(now)) && !self.shown_items().is_empty();

        let pass = cx.current_pass_size();
        let insets = cx.display_context.safe_area_insets;
        let window = window_bounds(pass, insets);

        if self.inline {
            draw_list.begin_overlay_reuse(cx);
            let main_walk = Walk {
                width: Size::Fixed(self.size),
                height: Size::Fixed(self.size),
                ..walk
            };
            // Positions read mid-draw are before alignment. The event side's
            // rect is the true one, and it holds while this draw sees the
            // button where the last one did.
            let seen = cx.peek_walk_turtle(main_walk);
            let honest = has_size(self.main_honest) && near(seen, self.main_seen);
            let centre = if honest { self.main_honest.center() } else { seen.center() };
            self.main_seen = seen;
            self.centre_used = centre;
            // The shadow and the actions float, on a root turtle: in the
            // parent's flow the shadow would be cut to the slot it stands in,
            // which is the button's own square, and its soft edge would end
            // in a hard one. Both go before the button, so they sit under it.
            cx.begin_root_turtle_for_pass(Layout::default());
            let face = Rect {
                pos: centre - self.size * 0.5,
                size: dvec2(self.size, self.size),
            };
            self.draw_main_shadow(cx, face);
            if dial_drawn {
                self.draw_items(cx, scope, centre, window);
            }
            cx.end_pass_sized_turtle();
            // The button in its parent's flow, so the parent's alignment
            // moves it with everything else.
            self.write_turn(cx, now);
            let _ = self.main.draw_walk(cx, scope, main_walk);
            draw_list.end(cx);
        } else {
            let whole = Rect {
                pos: dvec2(0.0, 0.0),
                size: pass,
            };
            // Whether the container is where the event side last read it,
            // and so whether that read's cut still holds.
            let mut read_holds = true;
            let mut ancestor_clip = (dvec2(f64::NEG_INFINITY, f64::NEG_INFINITY), dvec2(f64::INFINITY, f64::INFINITY));
            let host = match self.pin_to {
                FloatingPin::Window => whole,
                FloatingPin::Container => {
                    let seen = cx.turtle().rect_unscrolled();
                    // A container still measuring itself has no extent yet;
                    // the window is the only rect there is to pin to.
                    let seen = if seen.size.x.is_finite() && seen.size.y.is_finite() && has_size(seen) {
                        seen
                    } else {
                        whole
                    };
                    cx.add_aligned_rect_area(&mut self.host_area, seen);
                    ancestor_clip = cx.turtle_ancestor_clip();
                    let honest = has_size(self.host_honest) && near(seen, self.host_seen);
                    self.host_seen = seen;
                    read_holds = honest;
                    if honest {
                        self.host_honest
                    } else {
                        seen
                    }
                }
            };
            self.host_used = host;

            draw_list.begin_overlay_reuse(cx);
            cx.begin_root_turtle_for_pass(Layout::default());
            let bounds = pin_bounds(host, pass, insets);
            let face = main_rect(self.anchor, bounds, self.size, self.edge_margin);
            // Read on the event side, against the host where it was then. The
            // edges that hid it stay where they were, since a scroll moves
            // the host under them, and cut the host where it is now.
            //
            // Unless the container has moved since that read: a scroll step
            // that slides it under the view's edge is drawn before any event
            // reads it again, and the read then says it was whole. So on that
            // draw the cut comes from the turtles around the container, as
            // this draw has them, laid over the container where it is now.
            let cut = match (self.pin_to, self.host_visible) {
                (FloatingPin::Container, _) if !read_holds => {
                    let visible = host.clip(ancestor_clip);
                    (!near(visible, host)).then(|| (visible, cut_sides(visible, host, whole)))
                }
                (FloatingPin::Container, Some(visible)) => Some(cut_now(visible, self.host_honest, host, whole)),
                _ => None,
            };
            // The set is laid out inside what the cut leaves, so a container
            // half scrolled away folds its set into the part still on screen.
            let room = cut.map_or(window, |(_, open)| intersect(window, open));
            let clip = cut.map(|(visible, open)| {
                // With its button still in view, only the edges that hide the
                // container hide the set. With the button going out of view the
                // whole set goes with the container, rather than its actions
                // floating on over whatever the page scrolled under.
                if contains_rect(open, face) && contains_rect(visible, face) {
                    open
                } else {
                    visible
                }
            });
            if let Some(clip) = clip {
                cx.begin_turtle(Walk::fixed(clip.size.x, clip.size.y).with_abs_pos(clip.pos), Layout::default());
            }
            self.draw_main_shadow(cx, face);
            // The actions before the button, so they come out from under it.
            if dial_drawn {
                self.draw_items(cx, scope, face.center(), room);
            }
            self.write_turn(cx, now);
            let _ = self
                .main
                .draw_walk(cx, scope, Walk::fixed(self.size, self.size).with_abs_pos(face.pos));
            if clip.is_some() {
                cx.end_turtle();
            }
            cx.end_pass_sized_turtle();
            draw_list.end(cx);
            // Absolute in a root turtle, which no alignment moves.
            self.main_honest = face;
            self.centre_used = face.center();
        }
        self.draw_list = Some(draw_list);

        if self.open && self.trap_pending {
            // On a first open the trap was begun with an area from before the
            // actions were drawn, and the stops live in the draw just made.
            self.trap_pending = false;
            self.focus_trap.retarget(self.main.area());
        }
        if self.open && self.focus_first {
            self.focus_first = false;
            if let Some(first) = self.enabled_items().first().copied() {
                self.focus_item(cx, first);
            }
        }
        if self.inline || self.pin_to == FloatingPin::Container {
            self.check_frame = cx.new_next_frame();
        }
        DrawStep::done()
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(snapshot_text(self.open, self.pinned, self.dial, self.anchor))
    }
}

impl FloatingActionRef {
    pub fn open(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open(cx);
        }
    }

    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.close(cx);
        }
    }

    pub fn toggle(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.toggle(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.borrow().is_some_and(|inner| inner.is_open())
    }

    pub fn set_anchor(&self, cx: &mut Cx, anchor: FloatingAnchor) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_anchor(cx, anchor);
        }
    }

    pub fn set_dial(&self, cx: &mut Cx, dial: SpeedDialLayout) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_dial(cx, dial);
        }
    }

    pub fn set_item_enabled(&self, cx: &mut Cx, id: LiveId, enabled: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_item_enabled(cx, id, enabled);
        }
    }

    pub fn main_rect(&self) -> Rect {
        self.borrow().map(|inner| inner.main_rect()).unwrap_or_default()
    }

    fn has(&self, actions: &Actions, wanted: FloatingActionAction) -> bool {
        let uid = self.widget_uid();
        actions.iter().any(|action| {
            action
                .as_widget_action()
                .is_some_and(|wa| wa.widget_uid == uid && wa.cast::<FloatingActionAction>() == wanted)
        })
    }

    /// The button was clicked with no set behind it: the action itself.
    pub fn pressed(&self, actions: &Actions) -> bool {
        self.has(actions, FloatingActionAction::Pressed)
    }

    /// The action picked this pass, by its child id.
    pub fn picked(&self, actions: &Actions) -> Option<LiveId> {
        let uid = self.widget_uid();
        actions
            .iter()
            .filter_map(|action| action.as_widget_action())
            .filter(|wa| wa.widget_uid == uid)
            .find_map(|wa| match wa.cast::<FloatingActionAction>() {
                FloatingActionAction::Picked(id) => Some(id),
                _ => None,
            })
    }

    pub fn opened(&self, actions: &Actions) -> bool {
        self.has(actions, FloatingActionAction::Opened)
    }

    pub fn closed(&self, actions: &Actions) -> bool {
        self.has(actions, FloatingActionAction::Closed)
    }
}

#[cfg(test)]
mod tests {

    fn test_cx() -> crate::PooledCx {
        crate::checkout_test_cx()
    }
    use super::*;

    const BOUNDS: Rect = Rect {
        pos: DVec2 { x: 0.0, y: 0.0 },
        size: DVec2 { x: 800.0, y: 600.0 },
    };
    const MARGIN: Inset = Inset {
        left: 16.0,
        top: 16.0,
        right: 16.0,
        bottom: 16.0,
    };
    const MAIN: f64 = 56.0;
    const ITEM: f64 = 40.0;
    const GAP: f64 = 12.0;
    const MIN_R: f64 = 88.0;
    /// The widget's own furthest radius.
    const MAX_R: f64 = 200.0;
    /// A furthest radius of 0: the arcs are bounded by the window alone.
    const NO_MAX: f64 = 0.0;

    const LAYOUTS: [SpeedDialLayout; 3] = [
        SpeedDialLayout::Radial,
        SpeedDialLayout::Horizontal,
        SpeedDialLayout::Vertical,
    ];

    fn close_to(a: DVec2, b: DVec2, tolerance: f64) -> bool {
        (a.x - b.x).abs() <= tolerance && (a.y - b.y).abs() <= tolerance
    }

    fn no_insets() -> SafeAreaInsets {
        SafeAreaInsets {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        }
    }

    /// The button is `edge_margin` in from the edges its anchor touches and
    /// centred on an axis it does not touch. A wrong sign here pins the
    /// button off screen on exactly one of the eight, which a single anchor
    /// in a catalogue would never show.
    #[test]
    fn the_main_button_lands_at_each_anchor() {
        crate::on_test_cx(|| {
        for (anchor, centre) in [
            (FloatingAnchor::TopLeft, dvec2(44.0, 44.0)),
            (FloatingAnchor::TopCenter, dvec2(400.0, 44.0)),
            (FloatingAnchor::TopRight, dvec2(756.0, 44.0)),
            (FloatingAnchor::CenterLeft, dvec2(44.0, 300.0)),
            (FloatingAnchor::CenterRight, dvec2(756.0, 300.0)),
            (FloatingAnchor::BottomLeft, dvec2(44.0, 556.0)),
            (FloatingAnchor::BottomCenter, dvec2(400.0, 556.0)),
            (FloatingAnchor::BottomRight, dvec2(756.0, 556.0)),
        ] {
            let rect = main_rect(anchor, BOUNDS, MAIN, MARGIN);
            assert_eq!(rect.center(), centre, "{anchor:?}");
            assert_eq!(rect.size, dvec2(MAIN, MAIN), "{anchor:?}");
        }
        });
    }

    /// A container smaller than the button cannot hold it, and the button
    /// overruns to the far side rather than hanging off the near one, where
    /// its top would be under whatever sits above the container.
    #[test]
    fn a_container_smaller_than_the_button_keeps_its_low_edge() {
        crate::on_test_cx(|| {
        let tiny = Rect {
            pos: dvec2(10.0, 10.0),
            size: dvec2(40.0, 40.0),
        };
        let rect = main_rect(FloatingAnchor::BottomRight, tiny, MAIN, MARGIN);
        assert_eq!(rect.pos, dvec2(10.0, 10.0));
        });
    }

    /// A notch or a home indicator takes its insets out of the rect the
    /// button pins to, so a bottom corner button sits above the indicator
    /// rather than under it.
    #[test]
    fn pin_bounds_leave_the_safe_area() {
        crate::on_test_cx(|| {
        let insets = SafeAreaInsets {
            top: 40.0,
            bottom: 30.0,
            ..no_insets()
        };
        let bounds = pin_bounds(BOUNDS, BOUNDS.size, insets);
        assert_eq!(bounds, Rect {
            pos: dvec2(0.0, 40.0),
            size: dvec2(800.0, 530.0),
        });
        let rect = main_rect(FloatingAnchor::BottomRight, bounds, MAIN, MARGIN);
        assert_eq!(rect.center().y, 40.0 + 530.0 - 16.0 - 28.0);
        // A host entirely inside the safe area is untouched.
        let host = Rect {
            pos: dvec2(100.0, 100.0),
            size: dvec2(200.0, 200.0),
        };
        assert_eq!(pin_bounds(host, BOUNDS.size, insets), host);
        });
    }

    /// Every arc starts along one edge its anchor touches and ends along
    /// the other, running through the interior; the offsets it produces are
    /// the table's.
    #[test]
    fn the_arc_table_opens_inward() {
        crate::on_test_cx(|| {
        for (anchor, start, dir, span) in [
            (FloatingAnchor::BottomRight, 0.0, -1.0, 90.0),
            (FloatingAnchor::BottomLeft, 0.0, 1.0, 90.0),
            (FloatingAnchor::TopRight, 180.0, 1.0, 90.0),
            (FloatingAnchor::TopLeft, 180.0, -1.0, 90.0),
            (FloatingAnchor::CenterRight, 0.0, -1.0, 180.0),
            (FloatingAnchor::CenterLeft, 0.0, 1.0, 180.0),
            (FloatingAnchor::BottomCenter, 270.0, 1.0, 180.0),
            (FloatingAnchor::TopCenter, 270.0, -1.0, 180.0),
        ] {
            let (s, d, w) = radial_arc(anchor);
            assert!((s - f64::to_radians(start)).abs() < 1e-12, "{anchor:?} start");
            assert_eq!(d, dir, "{anchor:?} direction");
            assert!((w - f64::to_radians(span)).abs() < 1e-12, "{anchor:?} span");
        }
        let offsets = dial_offsets(FloatingAnchor::BottomRight, SpeedDialLayout::Radial, 3, MAIN, ITEM, GAP, MIN_R, NO_MAX);
        for (got, want) in offsets.iter().zip([
            dvec2(0.0, -88.0),
            dvec2(-62.23, -62.23),
            dvec2(-88.0, 0.0),
        ]) {
            assert!(close_to(*got, want, 0.01), "{got:?} is not {want:?}");
        }
        });
    }

    /// The arc grows before two actions' discs would touch, rather than
    /// the discs shrinking, and never closes in on the main button.
    #[test]
    fn the_fan_grows_before_neighbours_touch() {
        crate::on_test_cx(|| {
        let quarter = f64::to_radians(90.0);
        let half = f64::to_radians(180.0);
        assert!((radial_radius(4, quarter, ITEM, GAP, MAIN, MIN_R) - 100.46).abs() < 0.01);
        assert_eq!(radial_radius(3, quarter, ITEM, GAP, MAIN, MIN_R), 88.0);
        for span in [quarter, half] {
            for n in 2..=8usize {
                let r = radial_radius(n, span, ITEM, GAP, MAIN, MIN_R);
                let step = span / (n - 1) as f64;
                let chord = 2.0 * r * (step * 0.5).sin();
                assert!(chord >= ITEM + GAP - 1e-9, "n {n} span {span}: chord {chord}");
                assert!(r >= MAIN * 0.5 + GAP + ITEM * 0.5, "n {n} span {span}: radius {r}");
            }
        }
        });
    }

    /// Past the chord rule, an arc grows until no two squares on it meet and
    /// none meets the button's: a press is aimed at a square, and four on a
    /// quarter arc at the chord rule's radius share corners on the diagonal.
    #[test]
    fn the_arc_grows_before_neighbouring_squares_meet() {
        crate::on_test_cx(|| {
        let quarter = f64::to_radians(90.0);
        let half = f64::to_radians(180.0);
        assert!(arc_radius(4, quarter, ITEM, GAP, MAIN, MIN_R) > radial_radius(4, quarter, ITEM, GAP, MAIN, MIN_R));
        assert_eq!(arc_radius(3, quarter, ITEM, GAP, MAIN, MIN_R), 88.0, "the spec's three are untouched");
        for (item, gap, main, min_r) in [(ITEM, GAP, MAIN, MIN_R), (32.0, 8.0, 40.0, 64.0), (64.0, 0.0, 96.0, 48.0)] {
            for (anchor, span) in [(FloatingAnchor::BottomRight, quarter), (FloatingAnchor::TopCenter, half)] {
                for n in 1..=12usize {
                    let r = arc_radius(n, span, item, gap, main, min_r);
                    let offsets = dial_offsets(anchor, SpeedDialLayout::Radial, n, main, item, gap, min_r, NO_MAX);
                    for (i, a) in offsets.iter().enumerate() {
                        assert!((a.length() - r).abs() < 1e-9, "n {n}: one arc");
                        assert!(a.x.abs().max(a.y.abs()) >= (main + item) * 0.5 - 1e-9, "n {n}: {i} meets the button");
                        for b in &offsets[i + 1..] {
                            let d = *b - *a;
                            assert!(d.x.abs().max(d.y.abs()) >= item - 1e-9, "item {item} n {n}: squares meet at {a:?} and {b:?}");
                            assert!(d.length() >= item + gap - 1e-9, "item {item} n {n}: faces too close");
                        }
                    }
                }
            }
        }
        assert!(arc_step(ITEM, GAP) >= ITEM * std::f64::consts::SQRT_2);
        assert_eq!(arc_step(ITEM, 30.0), ITEM + 30.0);
        });
    }

    /// No action ever goes outward, past the edge the button is pinned to.
    /// A row or column along its own axis, and a single action on a corner
    /// arc, go strictly inward on both axes.
    #[test]
    fn every_offset_points_inward() {
        crate::on_test_cx(|| {
        for anchor in FloatingAnchor::ALL {
            let inward = interior(anchor);
            for dial in LAYOUTS {
                for n in 1..=6usize {
                    for offset in dial_offsets(anchor, dial, n, MAIN, ITEM, GAP, MIN_R, NO_MAX) {
                        let along = offset.x * inward.x + offset.y * inward.y;
                        assert!(along >= -1e-9, "{anchor:?} {dial:?} n {n}: {offset:?}");
                        let strict = anchor.is_corner()
                            && (dial != SpeedDialLayout::Radial || n == 1);
                        if strict {
                            assert!(along > 0.0, "{anchor:?} {dial:?} n {n}: {offset:?}");
                        }
                    }
                }
            }
        }
        });
    }

    /// One step is an action's diameter and the gap. A row with no inward
    /// run along its own axis is centred on the button, one step in.
    #[test]
    fn rows_and_columns_step_by_size_and_gap() {
        crate::on_test_cx(|| {
        let run = |anchor, dial| dial_offsets(anchor, dial, 3, MAIN, ITEM, GAP, MIN_R, NO_MAX);
        assert_eq!(
            run(FloatingAnchor::BottomRight, SpeedDialLayout::Horizontal),
            vec![dvec2(-60.0, 0.0), dvec2(-112.0, 0.0), dvec2(-164.0, 0.0)]
        );
        assert_eq!(
            run(FloatingAnchor::BottomRight, SpeedDialLayout::Vertical),
            vec![dvec2(0.0, -60.0), dvec2(0.0, -112.0), dvec2(0.0, -164.0)]
        );
        assert_eq!(
            run(FloatingAnchor::BottomCenter, SpeedDialLayout::Horizontal),
            vec![dvec2(-52.0, -60.0), dvec2(0.0, -60.0), dvec2(52.0, -60.0)]
        );
        assert_eq!(
            run(FloatingAnchor::CenterRight, SpeedDialLayout::Vertical),
            vec![dvec2(-60.0, -52.0), dvec2(-60.0, 0.0), dvec2(-60.0, 52.0)]
        );
        });
    }

    /// Chips go on the side that faces the interior, for all twenty-four
    /// anchor and layout pairs.
    #[test]
    fn chips_go_on_the_inside() {
        crate::on_test_cx(|| {
        use ChipSide::*;
        use FloatingAnchor::*;
        for (anchor, vertical, horizontal) in [
            (TopLeft, Right, Below),
            (TopCenter, Left, Below),
            (TopRight, Left, Below),
            (CenterLeft, Right, Above),
            (CenterRight, Left, Above),
            (BottomLeft, Right, Above),
            (BottomCenter, Left, Above),
            (BottomRight, Left, Above),
        ] {
            assert_eq!(chip_side(anchor, SpeedDialLayout::Vertical), vertical, "{anchor:?} column");
            assert_eq!(chip_side(anchor, SpeedDialLayout::Horizontal), horizontal, "{anchor:?} row");
            assert_eq!(chip_side(anchor, SpeedDialLayout::Radial), Outward, "{anchor:?} arc");
        }
        });
    }

    /// A chip pulled toward the window's edge stays inside it, whichever
    /// side it asked for, and an outward chip clears its disc by the gap.
    #[test]
    fn a_chip_stays_in_the_window() {
        crate::on_test_cx(|| {
        let window = Rect {
            pos: dvec2(6.0, 6.0),
            size: dvec2(300.0, 200.0),
        };
        let size = dvec2(80.0, 20.0);
        let left = chip_rect(dvec2(30.0, 100.0), ITEM, 8.0, size, ChipSide::Left, dvec2(0.0, -60.0), 0.0, window);
        assert_eq!(left.pos.x, 6.0, "pulled in from the left edge");
        let right = chip_rect(dvec2(100.0, 100.0), ITEM, 8.0, size, ChipSide::Right, dvec2(0.0, -60.0), 0.0, window);
        assert_eq!(right.pos, dvec2(128.0, 90.0), "beside the disc, centred on it");
        let up = chip_rect(dvec2(150.0, 100.0), ITEM, 8.0, size, ChipSide::Outward, dvec2(0.0, -60.0), 0.0, window);
        assert_eq!(up.pos, dvec2(110.0, 52.0), "straight out, its near edge a gap clear");
        // An action on an inner arc puts its chip out past the outer arc.
        let past = chip_rect(dvec2(150.0, 100.0), ITEM, 8.0, size, ChipSide::Outward, dvec2(0.0, -60.0), 30.0, window);
        assert_eq!(past.pos, dvec2(110.0, 22.0), "carried out past the arc beyond it");
        });
    }

    /// Pulled back inside the window, a chip that would land on its own
    /// action's face goes beside the face instead, on the side facing the
    /// interior when that covers nothing and on the other side when it would.
    #[test]
    fn a_chip_pulled_over_its_own_action_goes_beside_it() {
        crate::on_test_cx(|| {
        let window = Rect {
            pos: dvec2(6.0, 6.0),
            size: dvec2(400.0, 300.0),
        };
        let size = dvec2(60.0, 20.0);
        let centre = dvec2(200.0, 30.0);
        let up = dvec2(0.0, -150.0);
        let inward = interior(FloatingAnchor::BottomRight);
        let pulled = chip_rect(centre, ITEM, 8.0, size, ChipSide::Outward, up, 0.0, window);
        assert!(covers_disc(pulled, centre, ITEM * 0.5), "the edge pulls it over its face");

        let placed = place_chip(centre, ITEM, 8.0, size, ChipSide::Outward, up, 0.0, inward, window, &[(centre, ITEM)]);
        assert!(!covers_disc(placed, centre, ITEM * 0.5), "{placed:?}");
        assert!(inside(placed, window));
        assert_eq!(placed.pos.x + placed.size.x, centre.x - ITEM * 0.5 - 8.0, "beside it, on the side facing in");

        let neighbour = dvec2(140.0, 30.0);
        let placed = place_chip(centre, ITEM, 8.0, size, ChipSide::Outward, up, 0.0, inward, window, &[(centre, ITEM), (neighbour, ITEM)]);
        assert_eq!(placed.pos.x, centre.x + ITEM * 0.5 + 8.0, "the other side, clear of the neighbour");

        let low = dvec2(200.0, 200.0);
        assert_eq!(
            place_chip(low, ITEM, 8.0, size, ChipSide::Outward, up, 0.0, inward, window, &[(low, ITEM)]),
            chip_rect(low, ITEM, 8.0, size, ChipSide::Outward, up, 0.0, window),
            "with room, where it was asked for"
        );
        });
    }

    /// Once a row or a column has folded, a chip is carried out past the
    /// lines beside its own, or for lines centred on the button out on the
    /// nearer side of the set, so it covers no other action and not the
    /// button, from every anchor.
    #[test]
    fn a_chip_on_folded_lines_covers_no_other_action() {
        crate::on_test_cx(|| {
        let size = dvec2(90.0, 22.0);
        for anchor in FloatingAnchor::ALL {
            for (dial, w, h) in [(SpeedDialLayout::Horizontal, 300.0, 900.0), (SpeedDialLayout::Vertical, 900.0, 300.0)] {
                let (host, window) = small_window(w, h);
                let (centre, plan) = plan_in(anchor, dial, 12, host, window);
                let what = format!("{anchor:?} {dial:?}");
                assert!(plan.row_count() >= 2, "{what}: folded {:?}", plan.rows);
                let mut faces: Vec<(DVec2, f64)> = plan.offsets.iter().map(|offset| (centre + *offset, ITEM)).collect();
                faces.push((centre, MAIN));
                for (k, offset) in plan.offsets.iter().enumerate() {
                    let (side, beyond) = chip_place(anchor, dial, plan.rows[k], &plan, ITEM, GAP);
                    let rect = place_chip(centre + *offset, ITEM, 8.0, size, side, *offset, beyond, interior(anchor), window, &faces);
                    assert!(inside(rect, window), "{what}: chip {k} at {rect:?}");
                    for (j, (face, d)) in faces.iter().enumerate() {
                        assert!(!covers_disc(rect, *face, d * 0.5), "{what}: chip {k} ({side:?} {beyond}) covers face {j}");
                    }
                }
            }
        }
        // One line: beside it, on the inside, carried nowhere.
        let (host, window) = small_window(1400.0, 900.0);
        let (_, plan) = plan_in(FloatingAnchor::BottomRight, SpeedDialLayout::Vertical, 3, host, window);
        assert_eq!(chip_place(FloatingAnchor::BottomRight, SpeedDialLayout::Vertical, 0, &plan, ITEM, GAP), (ChipSide::Left, 0.0));
        });
    }

    /// A column's chips never meet, so Auto shows them all; an arc's or a
    /// row's would, so Auto shows the one being aimed at.
    #[test]
    fn auto_labels_resolve_by_layout() {
        crate::on_test_cx(|| {
        assert_eq!(labels_shown(SpeedDialLabels::Auto, SpeedDialLayout::Vertical), SpeedDialLabels::Always);
        assert_eq!(labels_shown(SpeedDialLabels::Auto, SpeedDialLayout::Radial), SpeedDialLabels::Hot);
        assert_eq!(labels_shown(SpeedDialLabels::Auto, SpeedDialLayout::Horizontal), SpeedDialLabels::Hot);
        for labels in [SpeedDialLabels::Always, SpeedDialLabels::Hot, SpeedDialLabels::Never] {
            for dial in LAYOUTS {
                assert_eq!(labels_shown(labels, dial), labels);
            }
        }
        // Folded into lines, a chip beside one line lies over the next, so
        // only the one aimed at shows; an arc's chips go out past the arcs.
        use SpeedDialLabels::*;
        assert_eq!(labels_shown_in(Always, SpeedDialLayout::Vertical, 2), Hot);
        assert_eq!(labels_shown_in(Auto, SpeedDialLayout::Vertical, 2), Hot);
        assert_eq!(labels_shown_in(Always, SpeedDialLayout::Horizontal, 3), Hot);
        assert_eq!(labels_shown_in(Auto, SpeedDialLayout::Vertical, 1), Always);
        assert_eq!(labels_shown_in(Always, SpeedDialLayout::Radial, 2), Always);
        assert_eq!(labels_shown_in(Never, SpeedDialLayout::Horizontal, 3), Never);
        });
    }

    /// Arrows go where the actions went. Along a column the arrow the
    /// column runs in steps outward and the other steps back, a sideways
    /// arrow has nowhere to go, and at the end of the column there is
    /// nothing further. On an arc two actions can be equally near, and the
    /// better aligned one is the one the arrow means.
    #[test]
    fn arrows_follow_the_direction_the_actions_went() {
        crate::on_test_cx(|| {
        let up = dvec2(0.0, -1.0);
        let down = dvec2(0.0, 1.0);
        let left = dvec2(-1.0, 0.0);
        let origin = dvec2(0.0, 0.0);
        let column = [dvec2(0.0, -60.0), dvec2(0.0, -112.0), dvec2(0.0, -164.0)];
        assert_eq!(arrow_target(&column, None, origin, up), Some(0));
        assert_eq!(arrow_target(&column, Some(0), origin, up), Some(1));
        assert_eq!(arrow_target(&column, Some(2), origin, up), None);
        assert_eq!(arrow_target(&column, Some(1), origin, down), Some(0));
        assert_eq!(arrow_target(&column, Some(1), origin, left), None);

        let arc = dial_offsets(FloatingAnchor::BottomRight, SpeedDialLayout::Radial, 3, MAIN, ITEM, GAP, MIN_R, NO_MAX);
        assert_eq!(arrow_target(&arc, None, origin, left), Some(2));
        assert_eq!(arrow_target(&arc, None, origin, up), Some(0));
        });
    }

    /// Each action sets off a stagger after the one before and eases out
    /// into place; with motion reduced every one is already there.
    #[test]
    fn items_arrive_in_turn() {
        crate::on_test_cx(|| {
        let (enter, stagger) = (0.2, 0.03);
        let ease = Ease::OutCubic;
        assert_eq!(enter_progress(0, 0.0, enter, stagger, &ease, false), 0.0);
        assert_eq!(enter_progress(2, 0.06, enter, stagger, &ease, false), 0.0);
        assert_eq!(enter_progress(2, 0.26, enter, stagger, &ease, false), 1.0);
        assert!((enter_progress(1, 0.13, enter, stagger, &ease, false) - 0.875).abs() < 1e-9);
        for (i, elapsed) in [(0usize, 0.0), (3, 0.01), (7, -1.0), (1, 0.13)] {
            assert_eq!(enter_progress(i, elapsed, enter, stagger, &ease, true), 1.0);
        }
        });
    }

    const COUNTS: [usize; 6] = [1, 2, 3, 5, 8, 12];

    /// A host of `w` by `h` filling the window, and the rect the actions
    /// must stay inside: the window a little in from its edges.
    fn small_window(w: f64, h: f64) -> (Rect, Rect) {
        let host = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(w, h),
        };
        (host, window_bounds(host.size, no_insets()))
    }

    /// The sizes a set is laid out with.
    #[derive(Clone, Copy, Debug)]
    struct Sizes {
        main: f64,
        item: f64,
        gap: f64,
        min_r: f64,
        max_r: f64,
        margin: Inset,
    }

    const DEFAULT_SIZES: Sizes = Sizes {
        main: MAIN,
        item: ITEM,
        gap: GAP,
        min_r: MIN_R,
        max_r: NO_MAX,
        margin: MARGIN,
    };

    /// The set of `n` laid out from `anchor` in `host`, and its button's
    /// centre.
    fn plan_in(anchor: FloatingAnchor, dial: SpeedDialLayout, n: usize, host: Rect, window: Rect) -> (DVec2, DialPlan) {
        plan_sized(DEFAULT_SIZES, anchor, dial, n, host, window)
    }

    fn plan_sized(s: Sizes, anchor: FloatingAnchor, dial: SpeedDialLayout, n: usize, host: Rect, window: Rect) -> (DVec2, DialPlan) {
        let centre = main_rect(anchor, host, s.main, s.margin).center();
        (centre, dial_plan(anchor, dial, n, s.main, s.item, s.gap, s.min_r, s.max_r, centre, window))
    }

    fn resting_rect(centre: DVec2, offset: DVec2) -> Rect {
        square(centre + offset, ITEM)
    }

    fn square(middle: DVec2, side: f64) -> Rect {
        Rect {
            pos: middle - side * 0.5,
            size: dvec2(side, side),
        }
    }

    fn inside(rect: Rect, window: Rect) -> bool {
        rect.pos.x >= window.pos.x - 1e-6
            && rect.pos.y >= window.pos.y - 1e-6
            && rect.pos.x + rect.size.x <= window.pos.x + window.size.x + 1e-6
            && rect.pos.y + rect.size.y <= window.pos.y + window.size.y + 1e-6
    }

    fn overlap(a: Rect, b: Rect) -> bool {
        a.pos.x < b.pos.x + b.size.x - 1e-6
            && b.pos.x < a.pos.x + a.size.x - 1e-6
            && a.pos.y < b.pos.y + b.size.y - 1e-6
            && b.pos.y < a.pos.y + a.size.y - 1e-6
    }

    /// Every action inside the window, clear of the button, no two faces
    /// closer than the gap, no two squares overlapping and none over the
    /// button's, and the arcs or lines filled in order.
    fn assert_laid_out(what: &str, centre: DVec2, plan: &DialPlan, n: usize, window: Rect) {
        assert_laid_out_sized(what, DEFAULT_SIZES, centre, plan, n, window)
    }

    fn assert_laid_out_sized(what: &str, s: Sizes, centre: DVec2, plan: &DialPlan, n: usize, window: Rect) {
        assert_eq!(plan.offsets.len(), n, "{what}");
        assert_eq!(plan.rows.len(), n, "{what}");
        assert!(plan.rows.windows(2).all(|w| w[0] <= w[1]), "{what}: rows out of order {:?}", plan.rows);
        let button = square(centre, s.main);
        for (i, offset) in plan.offsets.iter().enumerate() {
            let rect = square(centre + *offset, s.item);
            assert!(inside(rect, window), "{what}: action {i} at {rect:?} leaves {window:?}");
            assert!(offset.length() >= s.main * 0.5 + s.gap + s.item * 0.5 - 1e-6, "{what}: action {i} on the button");
            assert!(!overlap(rect, button), "{what}: action {i} over the button's square");
            for j in i + 1..n {
                let apart = (*offset - plan.offsets[j]).length();
                assert!(apart >= s.item + s.gap - 1e-6, "{what}: actions {i} and {j} are {apart} apart");
                assert!(!overlap(rect, square(centre + plan.offsets[j], s.item)), "{what}: squares {i} and {j} overlap");
            }
        }
    }

    /// Any number of actions, from every anchor, in every layout, in a
    /// window that holds twelve only by folding: none leaves the window and
    /// none overlaps another.
    #[test]
    fn any_number_of_actions_stay_in_the_window_and_apart() {
        crate::on_test_cx(|| {
        let (host, window) = small_window(480.0, 360.0);
        for anchor in FloatingAnchor::ALL {
            for dial in LAYOUTS {
                for n in COUNTS {
                    let (centre, plan) = plan_in(anchor, dial, n, host, window);
                    assert_laid_out(&format!("{anchor:?} {dial:?} n {n}"), centre, &plan, n, window);
                }
            }
        }
        });
    }

    /// A small button pinned close to the window's edge has less room
    /// between its middle and the edge than half an action, and the actions
    /// level with it reach that far: the set moves in by the difference and
    /// still leaves the window nowhere, at every count, from every anchor,
    /// in every layout.
    #[test]
    fn a_small_button_at_the_edge_keeps_its_set_in_the_window() {
        crate::on_test_cx(|| {
        let flush = Inset {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        };
        let small = Sizes {
            main: 40.0,
            item: 32.0,
            gap: 8.0,
            min_r: 64.0,
            max_r: NO_MAX,
            margin: flush,
        };
        let big_actions = Sizes {
            main: 40.0,
            item: 64.0,
            gap: 12.0,
            min_r: 88.0,
            max_r: NO_MAX,
            margin: MARGIN,
        };
        for (s, w, h) in [(small, 480.0, 360.0), (small, 1400.0, 900.0), (big_actions, 1400.0, 900.0)] {
            let (host, window) = small_window(w, h);
            for anchor in FloatingAnchor::ALL {
                for dial in LAYOUTS {
                    for n in 1..=12usize {
                        let (centre, plan) = plan_sized(s, anchor, dial, n, host, window);
                        assert_laid_out_sized(&format!("{s:?} {w}x{h} {anchor:?} {dial:?} n {n}"), s, centre, &plan, n, window);
                    }
                }
            }
        }
        // Where there is room the set is not moved at all.
        let (host, window) = small_window(1400.0, 900.0);
        let (_, plan) = plan_in(FloatingAnchor::BottomRight, SpeedDialLayout::Vertical, 3, host, window);
        assert_eq!(plan.offsets, dial_offsets(FloatingAnchor::BottomRight, SpeedDialLayout::Vertical, 3, MAIN, ITEM, GAP, MIN_R, NO_MAX));
        });
    }

    /// The window the live checks force: eight and twelve on a corner arc
    /// take a second arc, and twelve in a column from the bottom edge or in
    /// a row from a corner fold into lines, all still inside.
    #[test]
    fn a_crowded_set_takes_a_second_arc_or_a_second_line() {
        crate::on_test_cx(|| {
        let (host, window) = small_window(400.0, 300.0);
        for (anchor, dial, n) in [
            (FloatingAnchor::BottomRight, SpeedDialLayout::Radial, 8),
            (FloatingAnchor::BottomRight, SpeedDialLayout::Radial, 12),
            (FloatingAnchor::BottomCenter, SpeedDialLayout::Vertical, 12),
            (FloatingAnchor::TopLeft, SpeedDialLayout::Horizontal, 12),
        ] {
            let what = format!("{anchor:?} {dial:?} n {n}");
            let (centre, plan) = plan_in(anchor, dial, n, host, window);
            assert!(plan.row_count() >= 2, "{what}: one row {:?}", plan.rows);
            assert_laid_out(&what, centre, &plan, n, window);
        }
        // Three still take the single arc of the spec.
        let (centre, plan) = plan_in(FloatingAnchor::BottomRight, SpeedDialLayout::Radial, 3, host, window);
        assert_eq!(plan.row_count(), 1);
        assert_eq!(plan.offsets, dial_offsets(FloatingAnchor::BottomRight, SpeedDialLayout::Radial, 3, MAIN, ITEM, GAP, MIN_R, NO_MAX));
        assert!(plan.offsets.iter().all(|offset| inside(resting_rect(centre, *offset), window)));
        });
    }

    /// An arc takes all it can hold before the arc outside it takes any:
    /// every arc but the last could not hold one more action at the widest
    /// the window lets it be, with the arcs outside it still inside.
    #[test]
    fn the_inner_arc_fills_first() {
        crate::on_test_cx(|| {
        let (host, window) = small_window(400.0, 300.0);
        let anchor = FloatingAnchor::BottomRight;
        let (_, _, span) = radial_arc(anchor);
        let centre = main_rect(anchor, host, MAIN, MARGIN).center();
        let room = arc_room(anchor, centre, ITEM, window);
        for n in [8usize, 12] {
            let counts = arc_counts(anchor, n, MAIN, ITEM, GAP, MIN_R, NO_MAX, centre, window);
            assert_eq!(counts.iter().sum::<usize>(), n);
            let arcs = counts.len();
            assert!(arcs >= 2, "n {n}: {counts:?}");
            for (k, count) in counts.iter().enumerate().take(arcs - 1) {
                let widest = room - (arcs - 1 - k) as f64 * arc_step(ITEM, GAP);
                let one_more = arc_radius(count + 1, span, ITEM, GAP, MAIN, MIN_R);
                assert!(one_more > widest, "n {n}: arc {k} of {counts:?} had room for another");
            }
            let plan = dial_plan(anchor, SpeedDialLayout::Radial, n, MAIN, ITEM, GAP, MIN_R, NO_MAX, centre, window);
            for (k, count) in counts.iter().enumerate() {
                assert_eq!(plan.rows.iter().filter(|row| **row == k).count(), *count, "n {n}: arc {k}");
            }
            assert!(plan.radii.windows(2).all(|w| w[1] >= w[0] + arc_step(ITEM, GAP) - 1e-9), "n {n}: {:?}", plan.radii);
            assert!(*plan.radii.last().unwrap() <= room + 1e-9, "n {n}: the outer arc leaves the window");
        }
        });
    }

    /// How many actions rest on each arc or line, innermost first.
    fn per_row(plan: &DialPlan) -> Vec<usize> {
        (0..plan.row_count()).map(|k| plan.rows.iter().filter(|row| **row == k).count()).collect()
    }

    /// Held to a furthest radius, a set the arcs inside it can hold lies
    /// wholly inside it, from every anchor and at every count, no two
    /// squares overlapping and the arcs filled in order. Whether those arcs
    /// can hold the set is counted apart from the layout: the arcs a step
    /// apart from the least radius out to the furthest, each holding all
    /// its spacing allows.
    #[test]
    fn a_furthest_radius_holds_every_arc_inside_it() {
        crate::on_test_cx(|| {
        let mut spread = 0;
        for max_r in [MAX_R, 120.0, 156.0, 260.0] {
            for anchor in FloatingAnchor::ALL {
                let (_, _, span) = radial_arc(anchor);
                let step = arc_step(ITEM, GAP);
                let mut held_inside = 0;
                let mut r = arc_radius(1, span, ITEM, GAP, MAIN, MIN_R);
                while r <= max_r + 1e-9 {
                    held_inside += arc_capacity(r, span, ITEM, GAP, MAIN, MIN_R, usize::MAX);
                    r += step;
                }
                for n in 1..=16usize {
                    let what = format!("max {max_r} {anchor:?} n {n}");
                    let plan = dial_plan(anchor, SpeedDialLayout::Radial, n, MAIN, ITEM, GAP, MIN_R, max_r, dvec2(0.0, 0.0), UNBOUNDED);
                    assert_laid_out(&what, dvec2(0.0, 0.0), &plan, n, UNBOUNDED);
                    if n > held_inside {
                        continue;
                    }
                    for (i, offset) in plan.offsets.iter().enumerate() {
                        assert!(offset.length() <= max_r + 1e-9, "{what}: action {i} rests {} out", offset.length());
                    }
                    assert!(plan.radii.iter().all(|r| *r <= max_r + 1e-9), "{what}: {:?}", plan.radii);
                    if arc_radius(n, span, ITEM, GAP, MAIN, MIN_R) > max_r + 1e-9 {
                        spread += 1;
                    }
                }
            }
        }
        assert!(spread > 0, "no set here would have reached past its furthest radius on one arc");
        });
    }

    /// The small preset's sizes and furthest radius.
    const SMALL_SIZES: Sizes = Sizes {
        main: 40.0,
        item: 32.0,
        gap: 8.0,
        min_r: 64.0,
        max_r: 156.0,
        margin: MARGIN,
    };

    /// The large preset's sizes and furthest radius.
    const LARGE_SIZES: Sizes = Sizes {
        main: 72.0,
        item: 48.0,
        gap: 14.0,
        min_r: 108.0,
        max_r: 238.0,
        margin: MARGIN,
    };

    /// Each preset's furthest radius is the least a step of 2 reaches that
    /// holds twelve from a corner, the most the anchors page shows, so
    /// every count up to twelve rests inside it from every anchor. From a
    /// corner three keep their one arc at the least radius, eight take arcs
    /// of five and three and twelve arcs of five and seven. Left to a large
    /// window, eight of the default size would rest on one arc 252.6 out
    /// and twelve on one 396.5 out.
    #[test]
    fn the_default_furthest_radius_holds_twelve_from_a_corner() {
        crate::on_test_cx(|| {
        let (host, window) = small_window(1400.0, 900.0);
        let default = Sizes {
            max_r: MAX_R,
            ..DEFAULT_SIZES
        };
        let quarter = f64::to_radians(90.0);
        for s in [default, SMALL_SIZES, LARGE_SIZES] {
            let twelve = least_room(12, quarter, s.main, s.item, s.gap, s.min_r, 0.0);
            assert!(twelve <= s.max_r && s.max_r < twelve + 2.0, "{s:?}: twelve need {twelve}");
            for anchor in FloatingAnchor::ALL {
                for n in 1..=12usize {
                    let what = format!("{s:?} {anchor:?} n {n}");
                    let (centre, plan) = plan_sized(s, anchor, SpeedDialLayout::Radial, n, host, window);
                    assert_laid_out_sized(&what, s, centre, &plan, n, window);
                    for (i, offset) in plan.offsets.iter().enumerate() {
                        assert!(offset.length() <= s.max_r + 1e-9, "{what}: action {i} rests {} out", offset.length());
                    }
                }
            }
            for (n, counts) in [(3usize, vec![3usize]), (8, vec![5, 3]), (12, vec![5, 7])] {
                let (_, plan) = plan_sized(s, FloatingAnchor::BottomRight, SpeedDialLayout::Radial, n, host, window);
                assert_eq!(per_row(&plan), counts, "{s:?} n {n}: {:?}", plan.radii);
                if n == 3 {
                    assert!((plan.radii[0] - s.min_r).abs() < 1e-9, "{s:?}: {:?}", plan.radii);
                }
            }
        }
        for (n, r) in [(8usize, 252.62), (12, 396.48)] {
            let (_, wide) = plan_in(FloatingAnchor::BottomRight, SpeedDialLayout::Radial, n, host, window);
            assert_eq!(wide.row_count(), 1, "n {n}");
            assert!((wide.radii[0] - r).abs() < 0.01, "n {n}: {:?}", wide.radii);
        }
        });
    }

    /// Held to a furthest radius as to a small window, a set takes more
    /// than one arc exactly when one arc would reach past it, and an arc
    /// takes all it can hold before the arc outside it takes any: every arc
    /// but the last could not hold one more at the widest the furthest
    /// radius lets it be with the arcs outside it still inside. Twelve from
    /// a corner and sixteen from an edge fit inside the default.
    #[test]
    fn inside_a_furthest_radius_the_inner_arc_fills_first() {
        crate::on_test_cx(|| {
        let step = arc_step(ITEM, GAP);
        for (anchor, most) in [(FloatingAnchor::BottomRight, 12usize), (FloatingAnchor::TopCenter, 16)] {
            let (_, _, span) = radial_arc(anchor);
            for n in 1..=most {
                let what = format!("{anchor:?} n {n}");
                let counts = arc_counts(anchor, n, MAIN, ITEM, GAP, MIN_R, MAX_R, dvec2(0.0, 0.0), UNBOUNDED);
                assert_eq!(counts.iter().sum::<usize>(), n, "{what}");
                let one_arc_fits = arc_radius(n, span, ITEM, GAP, MAIN, MIN_R) <= MAX_R + 1e-9;
                assert_eq!(counts.len() == 1, one_arc_fits, "{what}: {counts:?}");
                let arcs = counts.len();
                for (k, count) in counts.iter().enumerate().take(arcs - 1) {
                    let widest = MAX_R - (arcs - 1 - k) as f64 * step;
                    let one_more = arc_radius(count + 1, span, ITEM, GAP, MAIN, MIN_R);
                    assert!(one_more > widest, "{what}: arc {k} of {counts:?} had room for another");
                }
                let plan = dial_plan(anchor, SpeedDialLayout::Radial, n, MAIN, ITEM, GAP, MIN_R, MAX_R, dvec2(0.0, 0.0), UNBOUNDED);
                assert_eq!(per_row(&plan), counts, "{what}");
                assert!(plan.rows.windows(2).all(|w| w[0] <= w[1]), "{what}: {:?}", plan.rows);
                assert!(plan.radii.iter().all(|r| *r <= MAX_R + 1e-9), "{what}: {:?}", plan.radii);
            }
        }
        });
    }

    /// A furthest radius of 0 is no limit: the layout is the one a radius
    /// past anything the set would reach gives, at every count, and eight
    /// from a corner go back to the one wide arc a large window allows,
    /// where the widget's own furthest radius takes two.
    #[test]
    fn a_furthest_radius_of_zero_is_no_limit() {
        crate::on_test_cx(|| {
        let (host, window) = small_window(1400.0, 900.0);
        let quarter = f64::to_radians(90.0);
        let laid = |max_r: f64, n: usize| {
            let s = Sizes {
                max_r,
                ..DEFAULT_SIZES
            };
            plan_sized(s, FloatingAnchor::BottomRight, SpeedDialLayout::Radial, n, host, window).1
        };
        for n in 1..=12usize {
            assert_eq!(laid(NO_MAX, n), laid(1.0e6, n), "n {n}");
        }
        let wide = laid(NO_MAX, 8);
        assert_eq!(wide.row_count(), 1);
        assert!((wide.radii[0] - arc_radius(8, quarter, ITEM, GAP, MAIN, MIN_R)).abs() < 1e-9, "{:?}", wide.radii);
        assert_eq!(laid(MAX_R, 8).row_count(), 2);
        assert_eq!(
            dial_offsets(FloatingAnchor::BottomRight, SpeedDialLayout::Radial, 8, MAIN, ITEM, GAP, MIN_R, NO_MAX),
            dial_offsets(FloatingAnchor::BottomRight, SpeedDialLayout::Radial, 8, MAIN, ITEM, GAP, MIN_R, 1.0e6)
        );
        });
    }

    /// Lowering the furthest radius never moves an action further out,
    /// past what the arcs inside it can hold as well: every count to
    /// sixteen, from a corner and from an edge, with each preset's sizes,
    /// at every furthest radius the Max radius control can set.
    #[test]
    fn lowering_the_furthest_radius_never_moves_the_set_out() {
        crate::on_test_cx(|| {
        let furthest = |offsets: Vec<DVec2>| offsets.iter().map(|offset| offset.length()).fold(0.0, f64::max);
        for s in [DEFAULT_SIZES, SMALL_SIZES, LARGE_SIZES] {
            for anchor in [FloatingAnchor::BottomRight, FloatingAnchor::TopCenter] {
                for n in 1..=16usize {
                    let mut nearer: Option<(f64, f64)> = None;
                    for twos in 1..=200 {
                        let max_r = twos as f64 * 2.0;
                        let reach = furthest(dial_offsets(anchor, SpeedDialLayout::Radial, n, s.main, s.item, s.gap, s.min_r, max_r));
                        if let Some((lower, was)) = nearer {
                            assert!(
                                reach >= was - 1e-9,
                                "{s:?} {anchor:?} n {n}: held to {lower} the set reaches {was}, held to {max_r} only {reach}"
                            );
                        }
                        nearer = Some((max_r, reach));
                    }
                }
            }
        }
        });
    }

    /// A set more than the arcs inside its furthest radius can hold keeps
    /// every action, each at its full size and none over another, and rests
    /// as near the button as its spacing allows: on the arcs of the least
    /// room past the furthest radius that holds it, never further out than
    /// arcs laid tight from the least radius, and each arc filled before the
    /// next. From a corner held to 156, nine take arcs of four and five
    /// 165.9 out, where arcs from the least radius out would leave one
    /// action alone on a third arc 201.1 out. In a window that holds the set
    /// only past the furthest radius, nothing leaves the window.
    #[test]
    fn a_set_too_large_for_its_furthest_radius_rests_as_near_as_it_can_past_it() {
        crate::on_test_cx(|| {
        let step = arc_step(ITEM, GAP);
        let (host, window) = small_window(1400.0, 900.0);
        for max_r in [156.0, 100.0, 40.0] {
            let s = Sizes {
                max_r,
                ..DEFAULT_SIZES
            };
            for anchor in FloatingAnchor::ALL {
                let (_, _, span) = radial_arc(anchor);
                let least = arc_radius(1, span, ITEM, GAP, MAIN, MIN_R);
                for n in [20usize, 30] {
                    let what = format!("max {max_r} {anchor:?} n {n}");
                    let (centre, plan) = plan_sized(s, anchor, SpeedDialLayout::Radial, n, host, window);
                    assert_laid_out(&what, centre, &plan, n, window);
                    assert!(plan.radii.iter().any(|r| *r > max_r + 1e-9), "{what}: squeezed inside {:?}", plan.radii);
                    // Arcs a step apart from the least radius out, each
                    // holding all its spacing allows, reach this far.
                    let mut tight = least;
                    let mut left = n;
                    loop {
                        left -= arc_capacity(tight, span, ITEM, GAP, MAIN, MIN_R, left);
                        if left == 0 {
                            break;
                        }
                        tight += step;
                    }
                    let outer = *plan.radii.last().unwrap();
                    assert!(outer <= tight + 1e-9, "{what}: the outer arc at {outer} is further out than tight arcs reach, {tight}");
                    let counts = per_row(&plan);
                    for k in 0..counts.len() - 1 {
                        let one_more = arc_radius(counts[k] + 1, span, ITEM, GAP, MAIN, MIN_R);
                        let tightest = least + k as f64 * step;
                        assert!(one_more > tightest, "{what}: arc {k} of {counts:?} had room for another");
                    }
                }
            }
        }
        let quarter = f64::to_radians(90.0);
        for (n, counts, reach) in [(9usize, vec![4usize, 5], 165.9), (12, vec![5, 7], 199.2), (16, vec![4, 5, 7], 222.4)] {
            let offsets = dial_offsets(FloatingAnchor::BottomRight, SpeedDialLayout::Radial, n, MAIN, ITEM, GAP, MIN_R, 156.0);
            let room = least_room(n, quarter, MAIN, ITEM, GAP, MIN_R, 156.0);
            let plan = dial_plan(FloatingAnchor::BottomRight, SpeedDialLayout::Radial, n, MAIN, ITEM, GAP, MIN_R, 156.0, dvec2(0.0, 0.0), UNBOUNDED);
            assert_eq!(per_row(&plan), counts, "n {n}: {:?}", plan.radii);
            let outer = *plan.radii.last().unwrap();
            assert!((outer - reach).abs() < 0.05, "n {n}: {:?}", plan.radii);
            assert!((room - reach).abs() < 0.05, "n {n}: the least room is {room}");
            // Held to that least room, the set is laid out the same.
            let at_least = dial_offsets(FloatingAnchor::BottomRight, SpeedDialLayout::Radial, n, MAIN, ITEM, GAP, MIN_R, room);
            assert_eq!(offsets, at_least, "n {n}");
        }
        // Sixteen from a corner of a window 310 high: the least room that
        // holds them, 222.4, is past the furthest radius and inside the 240
        // the window leaves.
        let (host, window) = small_window(1400.0, 310.0);
        let s = Sizes {
            max_r: MAX_R,
            ..DEFAULT_SIZES
        };
        let centre = main_rect(FloatingAnchor::BottomRight, host, MAIN, MARGIN).center();
        assert!((arc_room(FloatingAnchor::BottomRight, centre, ITEM, window) - 240.0).abs() < 1e-9);
        let (centre, plan) = plan_sized(s, FloatingAnchor::BottomRight, SpeedDialLayout::Radial, 16, host, window);
        assert_laid_out("sixteen under a low window", centre, &plan, 16, window);
        assert_eq!(per_row(&plan), vec![4, 5, 7], "{:?}", plan.radii);
        assert!((plan.radii[2] - 222.4).abs() < 0.05, "{:?}", plan.radii);
        });
    }

    /// A small button pinned flush to a corner or an edge moves its set in
    /// off the window's edge, and the furthest radius is still measured
    /// from the button's own middle: every set that fits inside it that far
    /// nearer rests inside it, where laying the arcs around the moved
    /// centre alone would have left some of them past it.
    #[test]
    fn a_set_moved_in_off_the_edge_keeps_inside_its_furthest_radius() {
        crate::on_test_cx(|| {
        let flush = Inset {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        };
        let (host, window) = small_window(1400.0, 900.0);
        let mut would_have_passed = 0;
        for anchor in FloatingAnchor::ALL {
            let (_, _, span) = radial_arc(anchor);
            for max_r in [150.0, 210.0, 266.0, 300.0] {
                let s = Sizes {
                    main: 40.0,
                    item: 64.0,
                    gap: 12.0,
                    min_r: 88.0,
                    max_r,
                    margin: flush,
                };
                for n in 1..=8usize {
                    let what = format!("{anchor:?} max {max_r} n {n}");
                    let (centre, plan) = plan_sized(s, anchor, SpeedDialLayout::Radial, n, host, window);
                    assert_laid_out_sized(&what, s, centre, &plan, n, window);
                    let unmoved = radial_plan(anchor, n, s.main, s.item, s.gap, s.min_r, max_r, centre, window);
                    let shift = pinned_overrun(anchor, &unmoved.offsets, centre, s.item, window);
                    if least_room(n, span, s.main, s.item, s.gap, s.min_r, 0.0) > max_r - shift.length() {
                        continue;
                    }
                    for (i, offset) in plan.offsets.iter().enumerate() {
                        assert!(offset.length() <= max_r + 1e-9, "{what}: action {i} rests {} from the button", offset.length());
                    }
                    let around_moved = radial_plan(anchor, n, s.main, s.item, s.gap, s.min_r, max_r, centre + shift, window);
                    if around_moved.offsets.iter().any(|offset| (*offset + shift).length() > max_r + 1e-9) {
                        would_have_passed += 1;
                    }
                }
            }
        }
        assert!(would_have_passed > 0, "no set here was moved past its furthest radius");
        });
    }

    /// The arrows walk an arc in order: from the button onto the inner arc,
    /// then one action at a time along it and on round the outer arc, never
    /// skipping one, and every step can be taken back. An arrow only ever
    /// moves the way it points, the step from the end of the inner arc back
    /// to the start of the outer one included. A folded row is walked by
    /// direction instead, across to the line beside it.
    #[test]
    fn arrows_walk_the_inner_arc_then_the_outer() {
        crate::on_test_cx(|| {
        let up = dvec2(0.0, -1.0);
        let down = dvec2(0.0, 1.0);
        let left = dvec2(-1.0, 0.0);
        let right = dvec2(1.0, 0.0);
        let arrows = [up, down, left, right];
        let origin = dvec2(0.0, 0.0);
        let (host, window) = small_window(400.0, 300.0);
        let (_, plan) = plan_in(FloatingAnchor::BottomRight, SpeedDialLayout::Radial, 12, host, window);
        let points = &plan.offsets;
        let first = plan.rows.iter().filter(|row| **row == 0).count();
        assert!(first < points.len(), "two arcs");
        assert_eq!(arc_walk_target(points, first, None, origin, up), Some(0), "up from the button: the top of the inner arc");
        assert_eq!(arc_walk_target(points, first, None, origin, left), Some(first - 1), "left: its other end");

        let mut walked = vec![0];
        let mut at = 0;
        while at + 1 < points.len() {
            for dir in arrows {
                let to = arc_walk_target(points, first, Some(at), origin, dir);
                assert!(to.is_none() || to == Some(at + 1) || to.map(|t| t + 1) == Some(at), "an arrow from {at} jumped to {to:?}");
            }
            let next = at + 1;
            assert!(
                arrows.iter().any(|dir| arc_walk_target(points, first, Some(at), origin, *dir) == Some(next)),
                "no arrow moves on from {at}"
            );
            assert!(
                arrows.iter().any(|dir| arc_walk_target(points, first, Some(next), origin, *dir) == Some(at)),
                "no arrow comes back to {at}"
            );
            walked.push(next);
            at = next;
        }
        assert_eq!(walked, (0..12).collect::<Vec<_>>());
        let rows: Vec<usize> = walked.iter().map(|i| plan.rows[*i]).collect();
        assert!(rows.windows(2).all(|w| w[0] <= w[1]), "the inner arc before the outer: {rows:?}");

        // From the left end of the inner arc the outer arc starts back at
        // the top: Down does not go up there, Up and Right do not go on down.
        let seam = first - 1;
        assert!(points[first].y < points[seam].y, "the outer arc starts above the inner arc's end");
        assert_ne!(arc_walk_target(points, first, Some(seam), origin, down), Some(first), "Down went up to the outer arc");
        assert_ne!(arc_walk_target(points, first, Some(first), origin, up), Some(seam), "Up went down to the inner arc");

        for (w, h, n) in [(400.0, 300.0, 8usize), (400.0, 300.0, 12), (1400.0, 360.0, 8), (1400.0, 360.0, 12), (1400.0, 420.0, 12)] {
            let (host, window) = small_window(w, h);
            for anchor in FloatingAnchor::ALL {
                let (_, plan) = plan_in(anchor, SpeedDialLayout::Radial, n, host, window);
                let points = &plan.offsets;
                let first = plan.rows.iter().filter(|row| **row == 0).count();
                let what = format!("{anchor:?} {w}x{h} n {n}");
                for at in 0..n {
                    for dir in arrows {
                        if let Some(to) = arc_walk_target(points, first, Some(at), origin, dir) {
                            assert!(to == at + 1 || to + 1 == at, "{what}: from {at} to {to}");
                            let step = points[to] - points[at];
                            assert!(step.x * dir.x + step.y * dir.y > 0.0, "{what}: from {at} the arrow {dir:?} went to {to}, against it");
                        }
                    }
                    if at + 1 < n {
                        assert!(arrows.iter().any(|dir| arc_walk_target(points, first, Some(at), origin, *dir) == Some(at + 1)), "{what}: stuck at {at}");
                        assert!(arrows.iter().any(|dir| arc_walk_target(points, first, Some(at + 1), origin, *dir) == Some(at)), "{what}: no way back to {at}");
                    }
                }
            }
        }

        let (_, grid) = plan_in(FloatingAnchor::TopLeft, SpeedDialLayout::Horizontal, 12, host, window);
        let per_line = grid.rows.iter().filter(|row| **row == 0).count();
        assert!(per_line < 12, "folded");
        assert_eq!(arrow_target(&grid.offsets, Some(0), origin, right), Some(1));
        assert_eq!(arrow_target(&grid.offsets, Some(0), origin, down), Some(per_line));
        assert_eq!(arrow_target(&grid.offsets, Some(per_line), origin, up), Some(0));
        });
    }

    /// Twelve open about as quickly as three: however many actions, the
    /// last one is in place no more than the budget after the first.
    #[test]
    fn a_large_set_opens_about_as_quickly_as_a_small_one() {
        crate::on_test_cx(|| {
        let (enter, stagger) = (0.2, 0.03);
        for n in 1..=48usize {
            let step = stagger_step(n, stagger);
            let last_in_place = enter + n.saturating_sub(1) as f64 * step;
            assert!(last_in_place <= enter + STAGGER_BUDGET + 1e-12, "{n}: {last_in_place}");
            if n <= 6 {
                assert!((step - stagger).abs() < 1e-12, "{n}: a small set keeps its stagger");
            }
            let legs = legs_to(&vec![0.0; n], 1.0, enter, step);
            assert!(legs.iter().all(|leg| leg.done(enter + STAGGER_BUDGET + 1e-9)), "{n}");
        }
        assert!(stagger_step(12, stagger) < stagger);
        });
    }

    /// Going away, the last action out is the first one back; and a set
    /// reopened part way back turns every action round at once, from where
    /// it is, with no jump.
    #[test]
    fn the_last_out_is_the_first_back_and_a_reopened_set_turns_round() {
        crate::on_test_cx(|| {
        let linear = Ease::Linear;
        let (exit, stagger) = (0.15, 0.03);
        let still_out: Vec<f64> = (0..3).map(|i| exit_progress(i, 3, 0.05, exit, stagger, &linear, false)).collect();
        assert!(still_out[2] < still_out[1] && still_out[1] < still_out[0], "{still_out:?}");
        let legs = legs_to(&[1.0, 1.0, 1.0], 0.0, exit, stagger);
        for (i, leg) in legs.iter().enumerate() {
            assert!((leg.delay - (2 - i) as f64 * stagger).abs() < 1e-12, "{i}: {leg:?}");
            assert!((leg.value(0.05, &linear) - still_out[i]).abs() < 1e-12, "{i}: the legs run the same curve");
        }

        let back_out = legs_to(&still_out, 1.0, 0.2, stagger);
        for (leg, from) in back_out.iter().zip(&still_out) {
            assert_eq!(leg.delay, 0.0, "none waits for its turn");
            assert_eq!(leg.value(0.0, &linear), *from, "no jump");
            assert_eq!(leg.value(0.2, &linear), 1.0);
            assert!((leg.secs - 0.2 * (1.0 - from)).abs() < 1e-12, "the share of the time its distance asks for");
        }
        assert_eq!(exit_progress(0, 3, 0.0, exit, stagger, &linear, true), 0.0, "reduced motion is home at once");
        });
    }

    /// The theme easing a token names, read from the running theme.
    fn theme_value(vm: &mut ScriptVm, name: &str) -> ScriptValue {
        let theme = vm.module(id!(theme));
        vm.bx.heap.value(theme, LiveId::from_str(name).into(), NoTrap)
    }

    const THEME_EASES: [&str; 8] = [
        "motion_ease_standard",
        "motion_ease_standard_decelerate",
        "motion_ease_standard_accelerate",
        "motion_ease_emphasized_decelerate",
        "motion_ease_emphasized_accelerate",
        "motion_ease_linear",
        "motion_ease_spring",
        "motion_ease_bounce",
    ];

    /// Travel follows the theme's own evaluation of each easing token, in
    /// and out; the defaults are the tokens the addendum names; and a
    /// spring may carry an action past its place.
    #[test]
    fn the_motion_runs_on_the_theme_eases() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let (enter, exit, stagger) = (0.2, 0.15, 0.03);
            for name in THEME_EASES {
                let value = theme_value(vm, name);
                let ease = Ease::script_from_value(vm, value);
                for x in [0.25, 0.5, 0.75] {
                    let want = ease.map(x);
                    let out = enter_progress(2, 2.0 * stagger + x * enter, enter, stagger, &ease, false);
                    assert!((out - want).abs() < 1e-9, "{name} out at {x}: {out}, the theme says {want}");
                    let back = exit_progress(0, 3, 2.0 * stagger + x * exit, exit, stagger, &ease, false);
                    assert!((back - (1.0 - want)).abs() < 1e-9, "{name} back at {x}: {back}");
                    // Turned round part way, out again and back again: each
                    // leg from where its action had got to, on the same curve
                    // over the share of the time its distance asks for.
                    let part_way = [0.3, 0.6, 0.9];
                    for (to, secs) in [(1.0, enter), (0.0, exit)] {
                        for (leg, from) in legs_to(&part_way, to, secs, stagger).iter().zip(part_way) {
                            assert_eq!(leg.delay, 0.0, "{name}: turned round at once");
                            let got = leg.value(x * leg.secs, &ease);
                            let expected = from + (to - from) * want;
                            assert!((got - expected).abs() < 1e-9, "{name} turned toward {to} at {x}: {got}, the theme says {expected}");
                        }
                    }
                }
                assert_eq!(enter_progress(0, 0.0, enter, stagger, &ease, false), 0.0, "{name} starts at the button");
                assert_eq!(enter_progress(0, enter, enter, stagger, &ease, false), 1.0, "{name} lands exactly");
            }

            let spring = theme_value(vm, "motion_ease_spring");
            let spring = Ease::script_from_value(vm, spring);
            let peak = (1..100)
                .map(|k| enter_progress(0, k as f64 * 0.01 * enter, enter, 0.0, &spring, false))
                .fold(0.0, f64::max);
            assert!(peak > 1.0, "a spring goes past its place: {peak}");

            let value = crate::script_eval!(vm, {use mod.widgets.* FloatingAction{}});
            let widget = FloatingAction::script_from_value(vm, value);
            let token = theme_value(vm, "motion_ease_emphasized_decelerate");
            assert_eq!(widget.enter_ease, Ease::script_from_value(vm, token));
            let token = theme_value(vm, "motion_ease_standard_accelerate");
            assert_eq!(widget.exit_ease, Ease::script_from_value(vm, token));
            assert_eq!(Some(widget.enter_secs), theme_value(vm, "motion_short_4").as_f64());
            assert_eq!(Some(widget.exit_secs), theme_value(vm, "motion_short_3").as_f64());
        });
        });
    }

    /// An overshoot moves the face and never the resting place: at rest a
    /// face is where the layout put it, past its place it stops at the
    /// window's edge, and the layout presses and arrows read has no idea
    /// how far along the travel is.
    #[test]
    fn an_overshoot_moves_the_face_not_the_resting_place() {
        crate::on_test_cx(|| {
        let (host, window) = small_window(400.0, 300.0);
        let (centre, plan) = plan_in(FloatingAnchor::BottomRight, SpeedDialLayout::Radial, 12, host, window);
        for (k, offset) in plan.offsets.iter().enumerate() {
            let rest = resting_rect(centre, *offset);
            let room = bounding(window, rest);
            let still = keep_inside(centre + *offset, ITEM, room);
            assert!(close_to(still, centre + *offset, 1e-9), "{k}: moved at rest");
            let over = keep_inside(centre + *offset * 1.4, ITEM, room);
            assert!(inside(Rect { pos: over - ITEM * 0.5, size: dvec2(ITEM, ITEM) }, window), "{k}: the overshoot is cut by the edge");
        }
        // A window too small for the set: the resting place overruns and
        // still is not moved.
        let tiny = Rect {
            pos: dvec2(6.0, 6.0),
            size: dvec2(60.0, 60.0),
        };
        let far = dvec2(-200.0, -200.0);
        let rest = resting_rect(dvec2(40.0, 40.0), far);
        assert!(close_to(keep_inside(dvec2(40.0, 40.0) + far, ITEM, bounding(tiny, rest)), dvec2(40.0, 40.0) + far, 1e-9));
        });
    }

    #[test]
    fn snapshot_value_spells_the_state() {
        crate::on_test_cx(|| {
        assert_eq!(
            snapshot_text(true, false, SpeedDialLayout::Radial, FloatingAnchor::BottomRight),
            "open radial bottom-right"
        );
        assert_eq!(
            snapshot_text(true, true, SpeedDialLayout::Vertical, FloatingAnchor::TopLeft),
            "pinned vertical top-left"
        );
        assert_eq!(
            snapshot_text(false, false, SpeedDialLayout::Horizontal, FloatingAnchor::CenterLeft),
            "closed horizontal center-left"
        );
        });
    }

    /// The source of the DSL between `from` and the first line that closes
    /// the macro, a brace in the first column.
    fn block<'a>(source: &'a str, from: &str) -> &'a str {
        let start = source.find(from).unwrap_or_else(|| panic!("no {from}"));
        let rest = &source[start..];
        let end = rest.find("\n}").unwrap_or(rest.len());
        &rest[..end]
    }

    /// A splatted variant is exported as a bare name, and TopLeft,
    /// TopCenter and BottomCenter already are; and the short prefix belongs
    /// to the property-panel controls. Neither may creep into this DSL or
    /// into the two faces in button.rs.
    #[test]
    fn no_enum_is_splatted_and_no_name_starts_with_fab() {
        crate::on_test_cx(|| {
        let own = block(include_str!("floating_action.rs"), "script_mod! {");
        let faces = block(include_str!("button.rs"), "mod.widgets.ButtonFloating = ");
        for (name, text) in [("floating_action.rs", own), ("button.rs", faces)] {
            assert!(text.contains("mod.widgets."), "{name}: the block was not found");
            assert!(!text.contains(concat!("splat", "(")), "{name} splats an enum");
            assert!(!text.contains(concat!("mod.widgets.", "Fab")), "{name} names a widget with the short prefix");
        }
        assert!(own.contains("FloatingAnchor.BottomRight"));
        });
    }

    /// A widget dropped with its set out cannot release its own lock, and
    /// a lock nobody holds turns every press in the window away. The next
    /// floating action to hear an event releases it.
    #[test]
    fn a_lock_left_by_a_dropped_widget_is_released() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let value = crate::script_eval!(vm, {use mod.widgets.* FloatingAction{
                one := FloatingActionItem{label: "One"}
            }});
            let mut dropped = FloatingAction::script_from_value(vm, value);
            let mut survivor = FloatingAction::script_from_value(vm, value);
            vm.with_cx_mut(|cx| {
                // As an open set holds it; opening itself reads the clock,
                // which a test has none of.
                dropped.open = true;
                dropped.lock(cx);
                assert!(dropped.locked);
                assert!(cx.sweep_lock_area().is_some());
                drop(dropped);
                assert!(cx.sweep_lock_area().is_some(), "nothing could release it on drop");
                survivor.handle_event(cx, &Event::Signal, &mut Scope::empty());
                assert_eq!(cx.sweep_lock_area(), None, "the next one released it");
            });
        });
        });
    }

    /// The DSL is markup the compiler never reads: the presets, the item's
    /// face, the two shaders and the enum defaults only exist once it runs.
    #[test]
    fn the_presets_build_and_the_shaders_compile() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            for name in ["DrawFloatingShadowBase", "DrawFloatingChipBase"] {
                let value = match name {
                    "DrawFloatingShadowBase" => crate::script_eval!(vm, {
                        mod.shader.test_compile_draw_source(mod.widgets.FloatingAction.draw_shadow, "glsl", false)
                    }),
                    _ => crate::script_eval!(vm, {
                        mod.shader.test_compile_draw_source(mod.widgets.FloatingActionItem.draw_chip, "glsl", false)
                    }),
                };
                let text = vm
                    .bx
                    .heap
                    .string_with(value, |_heap, text| text.to_string())
                    .expect("the compiler answers with source");
                assert!(!text.starts_with("ERRORS:"), "{name} did not compile: {text}");
            }

            let value = crate::script_eval!(vm, {use mod.widgets.* FloatingActionLg{
                upload := FloatingActionItem{label: "Upload" enabled: false}
                folder := FloatingActionItem{label: "Folder"}
                stray := Label{text: "not an action"}
            }});
            let widget = FloatingAction::script_from_value(vm, value);
            assert_eq!(widget.size, 72.0);
            assert_eq!(widget.radial_max_radius, LARGE_SIZES.max_r);
            assert_eq!(widget.anchor, FloatingAnchor::BottomRight);
            assert_eq!(widget.dial, SpeedDialLayout::Radial);
            assert_eq!(widget.labels, SpeedDialLabels::Auto);
            assert_eq!(widget.edge_margin.left, 16.0);
            assert!(!widget.main.is_empty(), "the main face was built");
            assert!(widget.main.borrow::<Button>().is_some(), "the main face is a button");
            let ids: Vec<LiveId> = widget.items.iter().map(|(id, _)| *id).collect();
            assert_eq!(ids, vec![live_id!(upload), live_id!(folder)], "declaration order, the stray left out");
            let upload = widget.items[0].1.borrow::<FloatingActionItem>().unwrap();
            assert_eq!(upload.label, "Upload");
            assert!(!upload.enabled);
            assert!(upload.button.borrow::<Button>().is_some(), "each action has its face");
            assert_eq!(snapshot_text(widget.open, widget.pinned, widget.dial, widget.anchor), "closed radial bottom-right");
        });
        });
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

        fn draw(&mut self, cx: &mut Cx, root: &WidgetRef) {
            let size = dvec2(600.0, 400.0);
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

    /// A 600 by 400 box with a column of three pinned to its bottom right,
    /// the last one disabled, drawn once. No travel, so a draw lands every
    /// action where it belongs.
    fn set_in_a_box() -> (crate::PooledCx, WidgetRef, Target) {
        let mut cx = test_cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.widgets.*
                View{
                    width: 600
                    height: 400
                    fab := FloatingAction{
                        dial: mod.widgets.SpeedDialLayout.Vertical
                        reduced_motion: true
                        one := FloatingActionItem{label: "One"}
                        two := FloatingActionItem{label: "Two"}
                        later := FloatingActionItem{label: "Later" enabled: false}
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        (cx, root, target)
    }

    /// As `set_in_a_box`, with motion, and so slow that nothing arrives
    /// while a test runs.
    fn slow_set_in_a_box() -> (crate::PooledCx, WidgetRef, Target) {
        let mut cx = test_cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.widgets.*
                View{
                    width: 600
                    height: 400
                    fab := FloatingAction{
                        dial: mod.widgets.SpeedDialLayout.Vertical
                        enter_secs: 60.0
                        exit_secs: 60.0
                        stagger_secs: 0.0
                        one := FloatingActionItem{label: "One"}
                        two := FloatingActionItem{label: "Two"}
                        later := FloatingActionItem{label: "Later" enabled: false}
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        (cx, root, target)
    }

    fn with_set<R>(root: &WidgetRef, cx: &Cx, read: impl FnOnce(&FloatingAction) -> R) -> R {
        let fab = root.widget(cx, ids!(fab));
        let inner = fab.borrow::<FloatingAction>().expect("fab is a floating action");
        read(&inner)
    }

    fn main_area_in(root: &WidgetRef, cx: &Cx) -> Area {
        let fab = root.widget(cx, ids!(fab));
        let inner = fab.borrow::<FloatingAction>().expect("fab is a floating action");
        inner.main.area()
    }

    fn main_area(root: &WidgetRef, cx: &Cx) -> Area {
        with_set(root, cx, |set| set.main.area())
    }

    fn action_area(root: &WidgetRef, cx: &Cx, i: usize) -> Area {
        with_set(root, cx, |set| set.item_button_area(i))
    }

    fn is_open(root: &WidgetRef, cx: &Cx) -> bool {
        with_set(root, cx, |set| set.open)
    }

    fn reports(actions: &Actions) -> Vec<FloatingActionAction> {
        actions
            .iter()
            .filter_map(|action| action.as_widget_action())
            .map(|action| action.cast::<FloatingActionAction>())
            .filter(|action| *action != FloatingActionAction::None)
            .collect()
    }

    /// Moves the key focus the way the event loop does between events.
    fn settle_focus(cx: &mut Cx) {
        cx.action(());
        cx.handle_actions();
    }

    /// One event to `root`, then the actions it raised, round after round
    /// as the event loop delivers them, with the focus settled after; the
    /// set's own reports from all of it.
    fn deliver(cx: &mut Cx, root: &WidgetRef, event: &Event) -> Vec<FloatingActionAction> {
        let mut reported = Vec::new();
        let mut actions = cx.capture_actions(|cx| root.handle_event(cx, event, &mut Scope::empty()));
        while !actions.is_empty() {
            reported.extend(reports(&actions));
            let round = Event::Actions(actions);
            actions = cx.capture_actions(|cx| root.handle_event(cx, &round, &mut Scope::empty()));
        }
        settle_focus(cx);
        reported
    }

    fn press(abs: DVec2) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            handled: std::cell::Cell::new(Area::Empty),
            time: 0.0,
        })
    }

    fn release(abs: DVec2) -> Event {
        Event::MouseUp(MouseUpEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            time: 0.0,
        })
    }

    fn move_to(abs: DVec2) -> Event {
        Event::MouseMove(MouseMoveEvent {
            abs,
            lock_delta: DVec2::default(),
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            handled: std::cell::Cell::new(Area::Empty),
            time: 0.0,
        })
    }

    /// A set out over a page with an ordinary button beside it to take the
    /// mouse and keep it. Pinned, so the press on that button does not put
    /// the set away: dismissal is a different rule, and these two tests are
    /// about the pointer.
    fn a_set_and_a_grabber() -> (crate::PooledCx, WidgetRef, Target) {
        let mut cx = test_cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.widgets.*
                View{
                    width: 600
                    height: 400
                    grabber := Button{width: 90. height: 32. text: "hold me"}
                    fab := FloatingAction{
                        pinned: true
                        dial: mod.widgets.SpeedDialLayout.Vertical
                        reduced_motion: true
                        one := FloatingActionItem{label: "One"}
                        two := FloatingActionItem{label: "Two"}
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        target.draw(&mut cx, &root);
        (cx, root, target)
    }

    /// The app-wide rule, at the place this set can break it. The actions
    /// are read from RAW moves — they have to be, because a travelling
    /// action's button hit-tests a place it has not reached yet — and a raw
    /// reader is told nothing about who holds the pointer. So a mouse a
    /// button elsewhere is holding used to light an action under it, and
    /// offer a press that the button's release was always going to cancel.
    #[test]
    fn an_action_does_not_light_under_a_mouse_another_control_holds() {
        crate::on_test_cx(|| {
        let (mut cx, root, _target) = a_set_and_a_grabber();
        assert!(is_open(&root, &cx), "a pinned set is out from its first draw");
        let over_one = action_area(&root, &cx, 0).rect(&cx).center();

        // Nobody holding: the pointer lights the action it is over.
        deliver(&mut cx, &root, &move_to(over_one));
        assert_eq!(with_set(&root, &cx, |set| set.hot_item), Some(0), "lit for a free pointer");

        // The button takes the mouse and keeps it until its release.
        let grabber = root.widget(&cx, ids!(grabber)).area().rect(&cx).center();
        deliver(&mut cx, &root, &press(grabber));
        assert!(cx.fingers.any_areas_captured(), "the button holds the mouse");
        deliver(&mut cx, &root, &move_to(over_one));
        assert_eq!(
            with_set(&root, &cx, |set| set.hot_item),
            None,
            "a pointer on loan to another control is not the set's to read"
        );

        // Let go, and it is the set's pointer again. A release drops the
        // hold in the platform's own mouse-up bookkeeping, which these tests
        // dispatch past — `unhandle` is that one step, by hand.
        let grabber_area = root.widget(&cx, ids!(grabber)).area();
        press(grabber).unhandle(&mut cx, &grabber_area);
        deliver(&mut cx, &root, &move_to(over_one));
        assert_eq!(
            with_set(&root, &cx, |set| set.hot_item),
            Some(0),
            "lit again once the hold is let go: the gate is the hold, not a mood"
        );
        });
    }

    /// And the other half, which is what keeps the set working at all: the
    /// set's OWN press is not somebody else's. Press, drag down the dial,
    /// release is one gesture, and the mouse is held by the button for the
    /// whole of it — so every action the hand travels over still lights up.
    #[test]
    fn the_sets_own_press_still_lights_the_actions_it_travels_over() {
        crate::on_test_cx(|| {
        let (mut cx, root, mut target) = set_in_a_box();
        let main = main_area(&root, &cx).rect(&cx).center();
        deliver(&mut cx, &root, &press(main));
        target.draw(&mut cx, &root);
        assert!(cx.fingers.any_areas_captured(), "the button holds the mouse for the gesture");
        let over_two = action_area(&root, &cx, 1).rect(&cx).center();
        deliver(&mut cx, &root, &move_to(over_two));
        assert_eq!(
            with_set(&root, &cx, |set| set.hot_item),
            Some(1),
            "the set's own press is not a press elsewhere"
        );
        });
    }

    fn key(key_code: KeyCode, shift: bool) -> Event {
        Event::KeyDown(KeyEvent {
            key_code,
            is_repeat: false,
            modifiers: KeyModifiers { shift, ..KeyModifiers::default() },
            time: 0.0,
        })
    }

    /// Press the button, drag to an action and let go: the set is out on
    /// the press, and the release picks, puts the set away and lets go of
    /// the pointer. One gesture, not a click and then another.
    #[test]
    fn a_press_opens_the_set_and_its_release_on_an_action_picks() {
        crate::on_test_cx(|| {
        let (mut cx, root, mut target) = set_in_a_box();
        let main = main_area(&root, &cx).rect(&cx).center();
        let reported = deliver(&mut cx, &root, &press(main));
        assert!(reported.contains(&FloatingActionAction::Opened), "{reported:?}");
        assert!(with_set(&root, &cx, |set| set.open && set.lock_pending), "out on the press, the grab owed to its release");
        target.draw(&mut cx, &root);
        let two = action_area(&root, &cx, 1).rect(&cx).center();
        let reported = deliver(&mut cx, &root, &release(two));
        assert!(reported.contains(&FloatingActionAction::Picked(live_id!(two))), "{reported:?}");
        assert!(reported.contains(&FloatingActionAction::Closed), "{reported:?}");
        assert!(!is_open(&root, &cx));
        assert_eq!(cx.sweep_lock_area(), None, "the pick let go of the pointer");
        });
    }

    /// Opened while the lock is still held from a dismissing press whose
    /// release has not come, the set keeps that grab and still reads the
    /// release as the end of its opening press.
    #[test]
    fn a_set_opened_under_a_held_lock_still_picks_on_the_release() {
        crate::on_test_cx(|| {
        let (mut cx, root, mut target) = set_in_a_box();
        let fab = root.widget(&cx, ids!(fab)).as_floating_action();
        fab.open(&mut cx);
        target.draw(&mut cx, &root);
        let main = main_area(&root, &cx).rect(&cx).center();
        deliver(&mut cx, &root, &press(dvec2(40.0, 40.0)));
        assert!(!is_open(&root, &cx) && with_set(&root, &cx, |set| set.locked), "closed, the grab kept for the release");
        let reported = deliver(&mut cx, &root, &press(main));
        assert!(reported.contains(&FloatingActionAction::Opened), "{reported:?}");
        assert!(with_set(&root, &cx, |set| set.opening_press), "the held grab does not make this press any less the opening one");
        target.draw(&mut cx, &root);
        let one = action_area(&root, &cx, 0).rect(&cx).center();
        let reported = deliver(&mut cx, &root, &release(one));
        assert!(reported.contains(&FloatingActionAction::Picked(live_id!(one))), "{reported:?}");
        });
    }

    /// A press that misses the set puts it away and is eaten: the grab
    /// stays until its release, so the dismissing click reaches nothing.
    #[test]
    fn a_press_outside_puts_the_set_away_and_its_release_reaches_nothing() {
        crate::on_test_cx(|| {
        let (mut cx, root, mut target) = set_in_a_box();
        root.widget(&cx, ids!(fab)).as_floating_action().open(&mut cx);
        target.draw(&mut cx, &root);
        let held = cx.sweep_lock_area();
        assert!(held.is_some(), "an open set holds the pointer");
        let outside = dvec2(40.0, 40.0);
        let reported = deliver(&mut cx, &root, &press(outside));
        assert!(reported.contains(&FloatingActionAction::Closed), "{reported:?}");
        assert_eq!(cx.sweep_lock_area(), held, "held until the release");
        deliver(&mut cx, &root, &release(outside));
        assert_eq!(cx.sweep_lock_area(), None);
        });
    }

    /// Return on the button brings the set out with the first action
    /// holding the keyboard; Escape puts it away and hands the keyboard
    /// back to the button.
    #[test]
    fn return_opens_on_the_first_action_and_escape_gives_the_button_back_the_keyboard() {
        crate::on_test_cx(|| {
        let (mut cx, root, mut target) = set_in_a_box();
        let focus = main_area(&root, &cx);
        cx.set_key_focus(focus);
        settle_focus(&mut cx);
        let reported = deliver(&mut cx, &root, &key(KeyCode::ReturnKey, false));
        assert!(reported.contains(&FloatingActionAction::Opened), "{reported:?}");
        target.draw(&mut cx, &root);
        settle_focus(&mut cx);
        assert!(cx.has_key_focus(action_area(&root, &cx, 0)), "the first action has the keyboard");
        let reported = deliver(&mut cx, &root, &key(KeyCode::Escape, false));
        assert!(reported.contains(&FloatingActionAction::Closed), "{reported:?}");
        assert!(cx.has_key_focus(main_area(&root, &cx)), "the button has it back");
        });
    }

    /// Tab and Shift+Tab go round the set's stops without resting on a
    /// disabled action, which Return could not pick.
    #[test]
    fn tab_passes_over_a_disabled_action() {
        crate::on_test_cx(|| {
        let (mut cx, root, mut target) = set_in_a_box();
        root.widget(&cx, ids!(fab)).as_floating_action().open(&mut cx);
        target.draw(&mut cx, &root);
        let two = action_area(&root, &cx, 1);
        cx.set_key_focus(two);
        settle_focus(&mut cx);
        deliver(&mut cx, &root, &key(KeyCode::Tab, false));
        assert!(!cx.has_key_focus(action_area(&root, &cx, 2)), "Tab rested on Later");
        assert!(cx.has_key_focus(main_area(&root, &cx)), "past Later to the button");
        deliver(&mut cx, &root, &key(KeyCode::Tab, true));
        assert!(cx.has_key_focus(two), "and back past it again");
        });
    }

    /// Pinned while it is out, a set lets go of the grab and the trap it
    /// took for the open: a pinned set never closes to give them back.
    #[test]
    fn pinning_an_open_set_lets_go_of_the_pointer() {
        crate::on_test_cx(|| {
        let (mut cx, root, mut target) = set_in_a_box();
        let mut fab = root.widget(&cx, ids!(fab));
        fab.as_floating_action().open(&mut cx);
        target.draw(&mut cx, &root);
        assert!(cx.sweep_lock_area().is_some());
        script_apply_eval!(cx, fab, {pinned: true});
        assert!(is_open(&root, &cx), "a pinned set stays out");
        assert!(with_set(&root, &cx, |set| !set.locked && !set.focus_trap.is_active()));
        assert_eq!(cx.sweep_lock_area(), None, "and holds no grab");
        });
    }

    /// An overlay that opened over the set holds the innermost lock. The
    /// set's own lifting and retaking around its children must not put its
    /// lock back on top of that one.
    #[test]
    fn a_lock_under_another_overlay_stays_under_it() {
        crate::on_test_cx(|| {
        let (mut cx, root, mut target) = set_in_a_box();
        root.widget(&cx, ids!(fab)).as_floating_action().open(&mut cx);
        target.draw(&mut cx, &root);
        // Any drawn area that is not the button stands in for the overlay.
        let above = action_area(&root, &cx, 0);
        cx.sweep_lock(above);
        root.handle_event(&mut cx, &Event::Signal, &mut Scope::empty());
        assert_eq!(cx.sweep_lock_area(), Some(above), "the overlay above is still on top");
        });
    }

    /// A container half scrolled under a view's top edge hides its set along
    /// that edge only: the set's actions reach past the container, and its
    /// container's other edges must not cut them off.
    #[test]
    fn only_the_edges_that_hide_the_container_cut_the_set() {
        crate::on_test_cx(|| {
        let window = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(800.0, 600.0),
        };
        let host = Rect {
            pos: dvec2(100.0, 20.0),
            size: dvec2(200.0, 200.0),
        };
        let visible = Rect {
            pos: dvec2(100.0, 60.0),
            size: dvec2(200.0, 160.0),
        };
        assert_eq!(cut_sides(visible, host, window), Rect {
            pos: dvec2(0.0, 60.0),
            size: dvec2(800.0, 540.0),
        });
        assert_eq!(cut_sides(host, host, window), window, "nothing hidden, nothing cut");
        let gone = Rect {
            pos: dvec2(100.0, 60.0),
            size: dvec2(200.0, 0.0),
        };
        assert_eq!(cut_sides(gone, host, window).size, dvec2(0.0, 0.0), "hidden entirely, nothing drawn");
        });
    }

    /// A scroll moves the container under the edge of the view it scrolls
    /// in, and not the edge: on the draw after a scroll step, before the
    /// container is read again, the set is still cut where that edge is.
    #[test]
    fn a_scrolled_container_is_cut_where_the_view_edge_is() {
        crate::on_test_cx(|| {
        let window = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(800.0, 600.0),
        };
        let host_then = Rect {
            pos: dvec2(100.0, 20.0),
            size: dvec2(200.0, 200.0),
        };
        let visible_then = Rect {
            pos: dvec2(100.0, 60.0),
            size: dvec2(200.0, 160.0),
        };
        let scrolled = Rect {
            pos: dvec2(100.0, -60.0),
            size: dvec2(200.0, 200.0),
        };
        let (visible, open) = cut_now(visible_then, host_then, scrolled, window);
        assert_eq!(open, Rect {
            pos: dvec2(0.0, 60.0),
            size: dvec2(800.0, 540.0),
        }, "the view's edge has not moved");
        assert_eq!(visible, Rect {
            pos: dvec2(100.0, 60.0),
            size: dvec2(200.0, 80.0),
        });
        let (visible, open) = cut_now(host_then, host_then, scrolled, window);
        assert_eq!(open, window, "nothing hidden then, nothing cut now");
        assert_eq!(visible, Rect {
            pos: dvec2(100.0, 0.0),
            size: dvec2(200.0, 140.0),
        });
        });
    }

    /// The first draw after a scroll step slides a container under the top
    /// edge of the view it scrolls in already cuts its set at that edge,
    /// with no event in between to read the container again. The set used
    /// to be cut only from the next read, so that one draw showed its
    /// button over whatever sat above the view: a toolbar, in the catalogue.
    #[test]
    fn a_container_scrolled_under_the_edge_is_cut_on_the_very_next_draw() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: 600
                    height: 400
                    flow: Down
                    bar := View{width: Fill height: 60}
                    scroller := View{
                        width: Fill
                        height: 300
                        flow: Down
                        box := View{
                            width: 400
                            height: 200
                            margin: Inset{top: 20}
                            fab := FloatingAction{
                                anchor: mod.widgets.FloatingAnchor.TopLeft
                                dial: mod.widgets.SpeedDialLayout.Vertical
                                reduced_motion: true
                                one := FloatingActionItem{label: "One"}
                                two := FloatingActionItem{label: "Two"}
                            }
                        }
                        filler := View{width: Fill height: 800}
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        root.handle_event(&mut cx, &Event::Signal, &mut Scope::empty());
        target.draw(&mut cx, &root);
        // Under the 60 point bar.
        let edge = 60.0;
        let face = main_area_in(&root, &cx).clipped_rect(&cx);
        assert!(face.pos.y >= edge, "nothing scrolled yet, and the button is below the edge: {face:?}");

        for step in 1..=3 {
            root.widget(&cx, ids!(scroller)).set_scroll_pos(&mut cx, dvec2(0.0, 20.0 * step as f64));
            root.redraw(&mut cx);
            target.draw(&mut cx, &root);
            let face = main_area_in(&root, &cx).clipped_rect(&cx);
            assert!(
                face.size.y <= 0.0 || face.pos.y >= edge - 0.5,
                "scroll step {step}: the button is drawn from {} above the view's edge at {edge}",
                face.pos.y
            );
            root.handle_event(&mut cx, &Event::Signal, &mut Scope::empty());
        }
        });
    }

    /// While the actions are still on their way, a press and release where
    /// one of them will rest picks it, though its face is still drawn next
    /// to the button: the hand aims at where the action is going.
    #[test]
    fn a_press_where_an_action_will_rest_picks_it_while_it_is_on_its_way() {
        crate::on_test_cx(|| {
        let (mut cx, root, mut target) = slow_set_in_a_box();
        root.widget(&cx, ids!(fab)).as_floating_action().open(&mut cx);
        target.draw(&mut cx, &root);
        let (resting, drawn) = with_set(&root, &cx, |set| (set.plan_centre + set.plan.offsets[1], set.item_button_area(1)));
        let drawn = drawn.rect(&cx).center();
        assert!((drawn - resting).length() > 40.0, "still near the button: drawn at {drawn:?}, resting at {resting:?}");
        assert!(with_set(&root, &cx, |set| set.travelling(cx.seconds_since_app_start())));
        let reported = deliver(&mut cx, &root, &press(resting));
        assert!(!reported.contains(&FloatingActionAction::Closed), "a press on a resting place is not a press outside: {reported:?}");
        let reported = deliver(&mut cx, &root, &release(resting));
        assert!(reported.contains(&FloatingActionAction::Picked(live_id!(two))), "{reported:?}");
        });
    }

    /// The plus turns on the set's own legs: out on the enter ease over the
    /// enter time, and when the set is put away before the turn got far, back
    /// from where it had got to on the exit ease. The button is told it is
    /// open at once, so its own track has nothing left to play over the top.
    #[test]
    fn the_glyph_turns_on_the_same_eases_as_the_set() {
        crate::on_test_cx(|| {
        let (mut cx, root, mut target) = slow_set_in_a_box();
        let fab = root.widget(&cx, ids!(fab)).as_floating_action();
        fab.open(&mut cx);
        with_set(&root, &cx, |set| {
            assert_eq!(set.turn, Leg { from: 0.0, to: 1.0, delay: 0.0, secs: set.enter_secs });
            assert_eq!(set.ease_toward(set.turn.to), set.enter_ease);
            let button = set.main.borrow::<Button>().expect("the main face is a button");
            assert!(button.open(), "the button's track is at its end");
        });
        target.draw(&mut cx, &root);
        fab.close(&mut cx);
        with_set(&root, &cx, |set| {
            assert_eq!(set.turn.to, 0.0);
            assert_eq!(set.ease_toward(set.turn.to), set.exit_ease);
            assert!(set.turn.from < 0.5, "back from where it had got to, not from the cross: {:?}", set.turn);
            assert!(set.moving(cx.seconds_since_app_start()), "the actions are still on their way back");
        });
        });
    }

    /// The set's own actions travel on its own eases, the theme's tokens by
    /// default: out along `enter_ease`, and, put away part way, back along
    /// `exit_ease` from wherever each had got to.
    #[test]
    fn the_actions_travel_on_the_sets_own_eases() {
        crate::on_test_cx(|| {
        let (mut cx, root, _target) = slow_set_in_a_box();
        let fab = root.widget(&cx, ids!(fab));
        fab.as_floating_action().open(&mut cx);
        {
            let mut set = fab.borrow_mut::<FloatingAction>().expect("a floating action");
            let (start, secs, ease) = (set.motion_started, set.enter_secs, set.enter_ease);
            for x in [0.25, 0.5, 0.75] {
                let at = set.positions(start + x * secs);
                assert!(at.iter().all(|v| (v - ease.map(x)).abs() < 1e-9), "out along the enter ease at {x}: {at:?}");
            }
            // Half the enter time gone, as far as the set's clock can tell.
            set.motion_started -= secs * 0.5;
        }
        fab.as_floating_action().close(&mut cx);
        let mut set = fab.borrow_mut::<FloatingAction>().expect("a floating action");
        let (start, enter, exit) = (set.motion_started, set.enter_ease, set.exit_ease);
        let legs = set.legs.clone();
        assert_eq!(legs.len(), 3);
        for leg in &legs {
            assert!((leg.from - enter.map(0.5)).abs() < 1e-3, "back from half way out: {leg:?}");
            assert_eq!((leg.to, leg.delay), (0.0, 0.0), "turned round at once: {leg:?}");
        }
        for x in [0.25, 0.5, 0.75] {
            let at = set.positions(start + x * legs[0].secs);
            for (v, leg) in at.iter().zip(&legs) {
                assert!((v - leg.from * (1.0 - exit.map(x))).abs() < 1e-9, "back along the exit ease at {x}: {v}");
            }
        }
        });
    }

    /// A hidden action is left out of the set: the others close up and
    /// take its place, a press on that place picks the one there now, and
    /// the hidden one is not drawn.
    #[test]
    fn a_hidden_action_leaves_no_gap() {
        crate::on_test_cx(|| {
        let (mut cx, root, mut target) = set_in_a_box();
        root.widget(&cx, ids!(fab)).as_floating_action().open(&mut cx);
        target.draw(&mut cx, &root);
        assert!(has_size(action_area(&root, &cx, 1).rect(&cx)), "Two is drawn while it is shown");
        root.widget(&cx, ids!(two)).set_visible(&mut cx, false);
        target.draw(&mut cx, &root);
        with_set(&root, &cx, |set| {
            assert_eq!(set.shown_items(), vec![0, 2]);
            assert_eq!(
                set.plan.offsets,
                dial_offsets(set.anchor, set.dial, 2, set.size, set.item_size, set.item_gap, set.radial_radius, set.radial_max_radius)
            );
            assert_eq!(set.item_at(set.plan_centre + set.plan.offsets[1]), Some(2), "Later rests where Two did");
        });
        assert!(!has_size(action_area(&root, &cx, 1).rect(&cx)), "Two is not drawn once hidden");
        });
    }

    /// Drawn with the widget's own furthest radius, eight from the corner
    /// of a box with room for one wide arc rest on two arcs inside that
    /// radius, and everything that reads the layout reads those two arcs:
    /// the arrow keys walk the inner arc and then the outer one, the chip of
    /// an action on the inner arc is carried out past the outer, and a press
    /// and release on each resting place picks the action resting there.
    #[test]
    fn a_set_held_inside_its_furthest_radius_is_walked_labelled_and_picked_where_it_rests() {
        crate::on_test_cx(|| {
        let mut cx = test_cx();
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.widgets.*
                View{
                    width: 600
                    height: 400
                    fab := FloatingAction{
                        reduced_motion: true
                        one := FloatingActionItem{label: "One"}
                        two := FloatingActionItem{label: "Two"}
                        three := FloatingActionItem{label: "Three"}
                        four := FloatingActionItem{label: "Four"}
                        five := FloatingActionItem{label: "Five"}
                        six := FloatingActionItem{label: "Six"}
                        seven := FloatingActionItem{label: "Seven"}
                        eight := FloatingActionItem{label: "Eight"}
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let ids = [
            live_id!(one),
            live_id!(two),
            live_id!(three),
            live_id!(four),
            live_id!(five),
            live_id!(six),
            live_id!(seven),
            live_id!(eight),
        ];
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        let fab = root.widget(&cx, ids!(fab)).as_floating_action();
        fab.open(&mut cx);
        target.draw(&mut cx, &root);
        let (max_r, plan, centre) = with_set(&root, &cx, |set| (set.radial_max_radius, set.plan.clone(), set.plan_centre));
        assert_eq!(max_r, MAX_R, "the widget's own furthest radius");
        assert_eq!(plan.rows, vec![0, 0, 0, 0, 0, 1, 1, 1], "{:?}", plan.radii);
        assert!(plan.offsets.iter().all(|offset| offset.length() <= max_r + 1e-9), "{:?}", plan.offsets);

        let origin = dvec2(0.0, 0.0);
        let arrows = [
            (KeyCode::ArrowUp, dvec2(0.0, -1.0)),
            (KeyCode::ArrowDown, dvec2(0.0, 1.0)),
            (KeyCode::ArrowLeft, dvec2(-1.0, 0.0)),
            (KeyCode::ArrowRight, dvec2(1.0, 0.0)),
        ];
        let focus = action_area(&root, &cx, 0);
        cx.set_key_focus(focus);
        settle_focus(&mut cx);
        for at in 0..7 {
            let (code, _) = arrows
                .iter()
                .find(|(_, dir)| arc_walk_target(&plan.offsets, 5, Some(at), origin, *dir) == Some(at + 1))
                .unwrap_or_else(|| panic!("no arrow moves on from {at}"));
            deliver(&mut cx, &root, &key(*code, false));
            assert!(cx.has_key_focus(action_area(&root, &cx, at + 1)), "{code:?} from {at} did not move on");
        }

        let focus = action_area(&root, &cx, 0);
        cx.set_key_focus(focus);
        settle_focus(&mut cx);
        target.draw(&mut cx, &root);
        let chip = root
            .widget(&cx, ids!(one))
            .borrow::<FloatingActionItem>()
            .and_then(|item| item.chip_rect)
            .expect("the focused action shows its chip");
        assert!(
            chip.pos.y + chip.size.y <= centre.y - plan.radii[1] - ITEM * 0.5 + 1e-6,
            "the chip at {chip:?} lies over the outer arc {:?}",
            plan.radii
        );

        for (k, id) in ids.iter().enumerate() {
            if !is_open(&root, &cx) {
                fab.open(&mut cx);
                target.draw(&mut cx, &root);
            }
            let resting = centre + plan.offsets[k];
            deliver(&mut cx, &root, &press(resting));
            let reported = deliver(&mut cx, &root, &release(resting));
            assert!(reported.contains(&FloatingActionAction::Picked(*id)), "action {k}: {reported:?}");
        }
        });
    }

    /// Shown for the first time while the set is out, an action that was
    /// never drawn has no area to redraw from; the set it joins is asked to
    /// redraw, or the action would not appear until something else redrew.
    #[test]
    fn an_action_shown_for_the_first_time_redraws_its_set() {
        crate::on_test_cx(|| {
        let (mut cx, root, mut target) = set_in_a_box();
        root.widget(&cx, ids!(two)).set_visible(&mut cx, false);
        root.widget(&cx, ids!(fab)).as_floating_action().open(&mut cx);
        target.draw(&mut cx, &root);
        assert!(action_area(&root, &cx, 1).is_empty(), "Two was never drawn");
        let list = with_set(&root, &cx, |set| set.draw_list.as_ref().unwrap().id());
        cx.new_draw_event.draw_lists.clear();
        cx.new_draw_event.redraw_all = false;
        root.widget(&cx, ids!(two)).set_visible(&mut cx, true);
        assert!(cx.new_draw_event.draw_lists.contains(&list), "the set was asked to redraw");
        target.draw(&mut cx, &root);
        assert!(has_size(action_area(&root, &cx, 1).rect(&cx)), "and Two is drawn");
        });
    }

    /// Hidden, the set is put away and its list comes off the screen: a
    /// view does not draw a hidden child, and the window's overlay drops a
    /// list whose parent was redrawn without it.
    #[test]
    fn a_hidden_set_comes_off_the_screen() {
        crate::on_test_cx(|| {
        let (mut cx, root, mut target) = set_in_a_box();
        let fab = root.widget(&cx, ids!(fab));
        fab.as_floating_action().open(&mut cx);
        target.draw(&mut cx, &root);
        let list = with_set(&root, &cx, |set| set.draw_list.as_ref().unwrap().id());
        let overlay = target.overlay.draw_list.id();
        let shown = |cx: &Cx| {
            let items = &cx.draw_lists[overlay].draw_items;
            (0..items.len()).any(|i| items[i].sub_list() == Some(list))
        };
        assert!(shown(&cx), "drawn on the window's overlay");
        fab.set_visible(&mut cx, false);
        assert!(!is_open(&root, &cx));
        assert_eq!(cx.sweep_lock_area(), None);
        target.draw(&mut cx, &root);
        assert!(!shown(&cx), "the overlay no longer shows it");
        });
    }
}
