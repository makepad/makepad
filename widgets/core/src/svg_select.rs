//! SvgSelect — a drawing that is the control.
//!
//! A map, a floor plan, a seating chart, a diagram of a machine: a picture
//! whose parts already mean something, where the thing being chosen has a
//! shape on screen and a list of names beside it would be a worse way of
//! saying the same thing. Each pickable part is named by the `id` the file
//! carries on a path or on a group, and that id is what comes back when one
//! is picked — so the drawing is the source of both the picture and the
//! vocabulary, and nothing has to be kept in step with it by hand.
//!
//! # It draws through the library's own SVG, and picks the same geometry
//!
//! There is no second parser and no second renderer here. The document is
//! the one the SVG draw layer already parses and retains, and the picking
//! walks that same node tree: each shape's segments are flattened once into
//! closed rings in the drawing's own coordinates, and a press is tested
//! against them with the shape's own fill rule. The test is therefore exact
//! rather than a box around the shape — the sea between two coastlines
//! belongs to neither of them.
//!
//! Lighting a region works the same way round. Instead of drawing a second
//! copy of the region over the first, the fill on the region's own nodes is
//! rewritten inside the retained document and the cached geometry is marked
//! stale. The picture is re-tessellated once per change of state — a press,
//! or the pointer crossing from one region to the next — and never per
//! frame.
//!
//! # Single and multiple
//!
//! `multiple: false` holds one region: a press moves the selection to
//! whatever was pressed. `multiple: true` toggles: a press adds a region,
//! and a press on one already held takes it away. Either way a press on a
//! part with no id, or on nothing at all, changes nothing and says nothing —
//! a miss on a picture is far more common than a miss on a button, and a
//! silent miss is better than one that clears the answer.
//!
//! # What the host may colour
//!
//! `set_tint` gives one id a colour of its own, which is what a map showing
//! a quantity per country needs. The tint replaces the fill the file was
//! authored with; hover and selection then mix on top of whichever of the
//! two is in force, so a tinted region still lights under the pointer.
//!
//! # What this deliberately does not do
//!
//! It does not pan, zoom or project: the drawing is fitted to the widget
//! once and stays there, and turning coordinates on a globe into coordinates
//! in a file is the business of whatever authored the file. It does not
//! label anything — the names are ids, not words for a reader, and the words
//! belong to the host. It does not hit-test strokes: a region is picked by
//! the area it fills, so a shape drawn as a bare outline is picked anywhere
//! inside that outline. `<use>` instances are drawn but never picked, since
//! the geometry they stand for lives in a symbol rather than in the node.
//! And a rounded rectangle is picked as a square one, which is a corner or
//! two of slack on a shape that is rarely a region at all.
use crate::{
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    widget_async::ScriptAsyncResult,
};

use crate::makepad_draw::svg::{
    viewbox_transform, SvgGroup, SvgNode, SvgPaint, SvgStyle, Transform2d,
};
use crate::makepad_draw::vector::{FillRule, PathCmd, VectorPath};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SvgSelectBase = #(SvgSelect::register_widget(vm))

    set_type_default() do #(DrawSvgSelect::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    /** A drawing whose named parts are pickable, on no ground of its own. */
    mod.widgets.SvgSelectFlat = set_type_default() do mod.widgets.SvgSelectBase{
        // A drawing needs room, and it is given real numbers rather than
        // Fill: the pages that show one put it in a Fit row, where a Fill
        // resolves to nothing at all.
        width: 520.
        height: 280.
        margin: theme.mspace_v_1

        /** hold more than one region at once */
        multiple: false
        /** how far the region under the pointer moves toward hover_color 0..1 step 0.05 */
        hover_weight: 0.35
        /** how far a held region moves toward select_color 0..1 step 0.05 */
        select_weight: 0.8
        /** how far the whole drawing flattens toward disabled_color when it is off 0..1 step 0.05 */
        disabled_weight: 0.7

        /** the ink the pointer lifts a region toward */
        hover_color: theme.color_primary
        /** the ink a held region is drawn in */
        select_color: theme.color_primary
        /** the ink a disabled drawing flattens toward */
        disabled_color: theme.color_surface_container_high

        draw_bg +: {
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: instance(0.0)
            /** disabled mix 0..1 step 0.01 */
            disabled: instance(0.0)

            /** ground edge thickness in pixels 0..4 step 0.5 */
            border_size: uniform(0.0)
            /** corner rounding radius 0..24 step 0.5 */
            border_radius: uniform(theme.corner_radius)
            /** focus ring thickness in pixels 0..6 step 0.5 */
            ring_size: uniform(theme.size_focus_ring)
            /** room between the drawing and its focus ring in pixels 0..16 step 0.5 */
            ring_gap: uniform(3.0)

            color: uniform(theme.color_u_hidden)
            border_color: uniform(theme.color_u_hidden)
            border_color_disabled: uniform(theme.color_u_hidden)
            ring_color: uniform(theme.color_primary)
            ring_color_off: uniform(theme.color_u_hidden)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                // The ground the drawing sits on.
                sdf.box(
                    self.border_size * 0.5
                    self.border_size * 0.5
                    max(self.rect_size.x - self.border_size, 1.0)
                    max(self.rect_size.y - self.border_size, 1.0)
                    self.border_radius
                )
                sdf.fill_keep(self.color)
                sdf.stroke(
                    self.border_color.mix(self.border_color_disabled, self.disabled)
                    self.border_size
                )

                // The focus ring goes round the DRAWING, not round the
                // widget. With the aspect kept the drawing leaves a margin
                // on two sides, and a ring round the margin would point at
                // room nothing is in.
                sdf.box(
                    self.art_pos.x - self.ring_gap
                    self.art_pos.y - self.ring_gap
                    self.art_size.x + self.ring_gap * 2.
                    self.art_size.y + self.ring_gap * 2.
                    self.border_radius
                )
                sdf.stroke(
                    self.ring_color_off.mix(self.ring_color, self.focus * (1.0 - self.disabled))
                    self.ring_size
                )

                return sdf.result
            }
        }

        animator: Animator{
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {focus: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {focus: 1.0}}
                }
            }
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {disabled: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {disabled: 1.0}}
                }
            }
        }
    }

    /** The standard one: the flat face on the theme's inset ground. */
    mod.widgets.SvgSelect = set_type_default() do mod.widgets.SvgSelectFlat{
        draw_bg +: {
            border_size: uniform(theme.beveling)
            color: uniform(theme.color_inset)
            border_color: uniform(theme.color_bevel_inset_1)
            border_color_disabled: uniform(theme.color_bevel_inset_1_disabled)
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSvgSelect {
    #[deref]
    draw_super: DrawQuad,
    /// The box the drawing actually covers inside the widget, relative to
    /// the widget's own corner. Rust owns it because the same fit decides
    /// where a press lands, and a ring drawn from a second opinion would
    /// disagree with the picture.
    #[live]
    art_pos: Vec2f,
    #[live]
    art_size: Vec2f,
}

// ---- The pure core: fitting a drawing into a box, and point-in-path ----

/// Where a drawing lands inside the box it is given, and the way back out of
/// it.
///
/// The SVG draw layer fits the drawing's own content bounds into the rect it
/// is handed, centring it on the axis with room to spare when the aspect is
/// kept. The hit test has to undo exactly that: any disagreement puts the
/// finger somewhere the eye is not.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Fit {
    /// The drawing's bounds in its own coordinates: (min_x, min_y, max_x, max_y).
    bounds: (f64, f64, f64, f64),
    /// The box it is drawn into.
    size: (f64, f64),
    keep_aspect: bool,
}

impl Fit {
    fn extent(&self) -> (f64, f64) {
        (self.bounds.2 - self.bounds.0, self.bounds.3 - self.bounds.1)
    }

    /// Points per document unit, on each axis.
    fn scale(&self) -> (f64, f64) {
        let (bw, bh) = self.extent();
        if bw <= 0.0 || bh <= 0.0 {
            return (1.0, 1.0);
        }
        let (tw, th) = self.size;
        if self.keep_aspect {
            let s = (tw / bw).min(th / bh);
            (s, s)
        } else {
            (tw / bw, th / bh)
        }
    }

    /// The box the drawing covers, relative to the widget's corner:
    /// (x, y, width, height).
    fn art(&self) -> (f64, f64, f64, f64) {
        let (bw, bh) = self.extent();
        if bw <= 0.0 || bh <= 0.0 {
            return (0.0, 0.0, self.size.0, self.size.1);
        }
        let (sx, sy) = self.scale();
        (
            (self.size.0 - bw * sx) * 0.5,
            (self.size.1 - bh * sy) * 0.5,
            bw * sx,
            bh * sy,
        )
    }

    /// A point in the widget, in the drawing's own coordinates.
    fn to_doc(&self, x: f64, y: f64) -> (f64, f64) {
        let (bw, bh) = self.extent();
        if bw <= 0.0 || bh <= 0.0 {
            return (x, y);
        }
        let (sx, sy) = self.scale();
        let (ax, ay, _, _) = self.art();
        (
            self.bounds.0 + (x - ax) / sx,
            self.bounds.1 + (y - ay) / sy,
        )
    }
}

/// One closed outline of a shape, in the drawing's own coordinates. A shape
/// with a hole in it is two rings; a shape in two pieces is two rings as
/// well, which is why the fill rule matters and a "ring count" does not.
type Ring = Vec<(f64, f64)>;

/// Which side of the directed edge a→b the point lies on: positive is left.
fn side_of(a: (f64, f64), b: (f64, f64), x: f64, y: f64) -> f64 {
    (b.0 - a.0) * (y - a.1) - (x - a.0) * (b.1 - a.1)
}

/// Does the area bounded by `rings` cover the point, under `rule`?
///
/// A ray is cast from the point along +x and the edges that cross it are
/// counted: their signed sum is the winding number and their plain count is
/// the parity. The straddle test is half-open in y — an edge counts when its
/// first end is at or below the ray and its second above, or the other way
/// about — so a vertex that lands exactly on the ray is counted once rather
/// than twice or not at all, which is what keeps a point level with a corner
/// from falling out of its own shape.
fn covers(rings: &[Ring], rule: FillRule, x: f64, y: f64) -> bool {
    let mut winding = 0i32;
    let mut crossings = 0i32;
    for ring in rings {
        // Two points bound no area, so there is nothing to be inside of.
        if ring.len() < 3 {
            continue;
        }
        for i in 0..ring.len() {
            let a = ring[i];
            let b = ring[(i + 1) % ring.len()];
            if a.1 <= y {
                if b.1 > y && side_of(a, b, x, y) > 0.0 {
                    winding += 1;
                    crossings += 1;
                }
            } else if b.1 <= y && side_of(a, b, x, y) < 0.0 {
                winding -= 1;
                crossings += 1;
            }
        }
    }
    match rule {
        FillRule::NonZero => winding != 0,
        FillRule::EvenOdd => crossings % 2 != 0,
    }
}

/// The bounds of a set of rings, or nothing when there are no points.
fn ring_bounds(rings: &[Ring]) -> Option<(f64, f64, f64, f64)> {
    let mut out: Option<(f64, f64, f64, f64)> = None;
    for ring in rings {
        for &(x, y) in ring {
            out = Some(match out {
                None => (x, y, x, y),
                Some(b) => (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y)),
            });
        }
    }
    out
}

/// Chop a cubic into straight pieces, appending them to `out`.
///
/// The piece count comes from the control polygon's length measured against
/// `unit` — a hundredth of the drawing, so a file drawn in a 24-unit box and
/// one drawn in a 1000-unit box are flattened equally finely rather than one
/// of them being flattened into a straight line.
fn flatten_cubic(
    from: (f64, f64),
    c1: (f64, f64),
    c2: (f64, f64),
    to: (f64, f64),
    unit: f64,
    out: &mut Ring,
) {
    let len = dist(from, c1) + dist(c1, c2) + dist(c2, to);
    let steps = if unit > 0.0 {
        ((len / unit).ceil() as usize).clamp(1, 32)
    } else {
        8
    };
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let u = 1.0 - t;
        let x = u * u * u * from.0 + 3.0 * u * u * t * c1.0 + 3.0 * u * t * t * c2.0 + t * t * t * to.0;
        let y = u * u * u * from.1 + 3.0 * u * u * t * c1.1 + 3.0 * u * t * t * c2.1 + t * t * t * to.1;
        out.push((x, y));
    }
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
}

fn xf_point(xf: &Transform2d, x: f32, y: f32) -> (f64, f64) {
    let (tx, ty) = xf.apply(x, y);
    (tx as f64, ty as f64)
}

/// Flatten a path into closed rings, in the drawing's own coordinates.
///
/// Every subpath is closed whether the file closed it or not: an open
/// subpath still gets filled by a renderer, so an open subpath still has an
/// inside to be pressed.
fn rings_of_path(path: &VectorPath, xf: &Transform2d, unit: f64) -> Vec<Ring> {
    let mut rings = Vec::new();
    let mut cur: Ring = Vec::new();
    let mut pen = (0.0, 0.0);
    let mut start = (0.0, 0.0);
    for cmd in &path.cmds {
        match cmd {
            PathCmd::MoveTo(x, y) => {
                push_ring(&mut rings, &mut cur);
                pen = xf_point(xf, *x, *y);
                start = pen;
                cur.push(pen);
            }
            PathCmd::LineTo(x, y) => {
                pen = xf_point(xf, *x, *y);
                cur.push(pen);
            }
            PathCmd::BezierTo(c1x, c1y, c2x, c2y, x, y) => {
                let c1 = xf_point(xf, *c1x, *c1y);
                let c2 = xf_point(xf, *c2x, *c2y);
                let to = xf_point(xf, *x, *y);
                flatten_cubic(pen, c1, c2, to, unit, &mut cur);
                pen = to;
            }
            PathCmd::Close => {
                push_ring(&mut rings, &mut cur);
                pen = start;
            }
            PathCmd::Winding(_) => {}
        }
    }
    push_ring(&mut rings, &mut cur);
    rings
}

fn push_ring(rings: &mut Vec<Ring>, cur: &mut Ring) {
    if cur.len() >= 3 {
        rings.push(std::mem::take(cur));
    } else {
        cur.clear();
    }
}

/// An ellipse as a ring. A circle is one of these with equal radii.
fn ring_of_ellipse(cx: f32, cy: f32, rx: f32, ry: f32, xf: &Transform2d) -> Ring {
    const SIDES: usize = 48;
    let mut ring = Ring::with_capacity(SIDES);
    for i in 0..SIDES {
        let a = i as f64 / SIDES as f64 * std::f64::consts::TAU;
        ring.push(xf_point(
            xf,
            cx + rx * a.cos() as f32,
            cy + ry * a.sin() as f32,
        ));
    }
    ring
}

// ---- The document, read for regions and written for colour ----

/// One filled shape of the drawing, in draw order.
///
/// The shapes are kept flat and in the order they are painted rather than
/// grouped under their region, because that order IS the depth order: the
/// press has to be answered by the shape the eye sees on top, and two
/// regions may well be interleaved.
#[derive(Clone, Debug)]
struct Shape {
    region: usize,
    rings: Vec<Ring>,
    rule: FillRule,
    bounds: (f64, f64, f64, f64),
}

impl Shape {
    fn covers(&self, x: f64, y: f64) -> bool {
        if x < self.bounds.0 || y < self.bounds.1 || x > self.bounds.2 || y > self.bounds.3 {
            return false;
        }
        covers(&self.rings, self.rule, x, y)
    }
}

/// One drawable node of the document, in the order the renderer walks them.
/// Every node is here, region or not: the ones with no region still have to
/// be dimmed when the control is off, and the counter has to stay in step
/// with the walk that writes the colours back.
#[derive(Clone, Debug)]
struct Leaf {
    region: Option<usize>,
    /// The fill the file was authored with, so a region that stops being
    /// lit goes back to what it was rather than to a guess.
    base: Option<SvgPaint>,
}

fn node_id(node: &SvgNode) -> Option<&str> {
    match node {
        SvgNode::Group(n) => n.id.as_deref(),
        SvgNode::Path(n) => n.id.as_deref(),
        SvgNode::Rect(n) => n.id.as_deref(),
        SvgNode::Circle(n) => n.id.as_deref(),
        SvgNode::Ellipse(n) => n.id.as_deref(),
        SvgNode::Line(n) => n.id.as_deref(),
        SvgNode::Polyline(n) => n.id.as_deref(),
        SvgNode::Polygon(n) => n.id.as_deref(),
        SvgNode::Use(n) => n.id.as_deref(),
    }
}

fn node_transform(node: &SvgNode) -> &Transform2d {
    match node {
        SvgNode::Group(n) => &n.transform,
        SvgNode::Path(n) => &n.transform,
        SvgNode::Rect(n) => &n.transform,
        SvgNode::Circle(n) => &n.transform,
        SvgNode::Ellipse(n) => &n.transform,
        SvgNode::Line(n) => &n.transform,
        SvgNode::Polyline(n) => &n.transform,
        SvgNode::Polygon(n) => &n.transform,
        SvgNode::Use(n) => &n.transform,
    }
}

fn node_style(node: &SvgNode) -> &SvgStyle {
    match node {
        SvgNode::Group(n) => &n.style,
        SvgNode::Path(n) => &n.style,
        SvgNode::Rect(n) => &n.style,
        SvgNode::Circle(n) => &n.style,
        SvgNode::Ellipse(n) => &n.style,
        SvgNode::Line(n) => &n.style,
        SvgNode::Polyline(n) => &n.style,
        SvgNode::Polygon(n) => &n.style,
        SvgNode::Use(n) => &n.style,
    }
}

fn node_style_mut(node: &mut SvgNode) -> &mut SvgStyle {
    match node {
        SvgNode::Group(n) => &mut n.style,
        SvgNode::Path(n) => &mut n.style,
        SvgNode::Rect(n) => &mut n.style,
        SvgNode::Circle(n) => &mut n.style,
        SvgNode::Ellipse(n) => &mut n.style,
        SvgNode::Line(n) => &mut n.style,
        SvgNode::Polyline(n) => &mut n.style,
        SvgNode::Polygon(n) => &mut n.style,
        SvgNode::Use(n) => &mut n.style,
    }
}

/// The rings a leaf fills, or nothing when it fills nothing.
fn rings_of_node(node: &SvgNode, xf: &Transform2d, unit: f64) -> Vec<Ring> {
    match node {
        SvgNode::Path(n) => rings_of_path(&n.path, xf, unit),
        SvgNode::Rect(n) => {
            // Rounded corners are picked square: the slack is a corner or
            // two on a shape that is rarely a region in the first place.
            vec![vec![
                xf_point(xf, n.x, n.y),
                xf_point(xf, n.x + n.width, n.y),
                xf_point(xf, n.x + n.width, n.y + n.height),
                xf_point(xf, n.x, n.y + n.height),
            ]]
        }
        SvgNode::Circle(n) => vec![ring_of_ellipse(n.cx, n.cy, n.r, n.r, xf)],
        SvgNode::Ellipse(n) => vec![ring_of_ellipse(n.cx, n.cy, n.rx, n.ry, xf)],
        // Two arms, not one: a polygon and a polyline are different types
        // even though both are a run of points.
        SvgNode::Polygon(n) => {
            vec![n.points.iter().map(|&(x, y)| xf_point(xf, x, y)).collect()]
        }
        SvgNode::Polyline(n) => {
            vec![n.points.iter().map(|&(x, y)| xf_point(xf, x, y)).collect()]
        }
        // A line encloses nothing, and a use stands for geometry that lives
        // in a symbol rather than in the node. A group holds no geometry of
        // its own — the walk reaches its children separately.
        SvgNode::Line(_) | SvgNode::Use(_) | SvgNode::Group(_) => Vec::new(),
    }
}

/// Visit every drawable node in the order the renderer paints them, handing
/// each one the id that owns it — its own, or the nearest enclosing group's
/// — and the transform it is drawn under.
///
/// One walk serves both the reading and the writing so the two cannot drift:
/// the index a node is given when its geometry is taken is the index it is
/// found at when its colour is put back.
fn walk_leaves(
    nodes: &mut [SvgNode],
    owner: Option<&str>,
    parent_xf: &Transform2d,
    index: &mut usize,
    visit: &mut dyn FnMut(usize, Option<&str>, &Transform2d, &mut SvgNode),
) {
    for node in nodes.iter_mut() {
        if let SvgNode::Group(group) = node {
            let SvgGroup { id, transform, children, .. } = group;
            let xf = transform.then(parent_xf);
            let inner = id.as_deref().or(owner);
            walk_leaves(children, inner, &xf, index, &mut *visit);
            continue;
        }
        // A node's own id wins over the group's, so a group can name a
        // region and one child of it can still name itself.
        let own = node_id(node).map(str::to_string);
        let owned_by = own.as_deref().or(owner);
        let xf = node_transform(node).then(parent_xf);
        let at = *index;
        *index += 1;
        visit(at, owned_by, &xf, node);
    }
}

/// The colour a fill is, when it is a plain colour at all. A gradient has no
/// single colour to mix with, and there is nothing to mix into `none`.
fn solid(paint: &Option<SvgPaint>) -> Option<[f32; 4]> {
    match paint {
        Some(SvgPaint::Color(r, g, b, a)) => Some([*r, *g, *b, *a]),
        _ => None,
    }
}

fn mix(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

fn rgba(c: Vec4f) -> [f32; 4] {
    [c.x, c.y, c.z, c.w]
}

#[derive(Clone, Debug, Default)]
pub enum SvgSelectAction {
    /// A press landed on a region: its id, and the whole selection as it
    /// stands afterwards.
    Picked(String, Vec<String>),
    /// The pointer moved onto a region.
    Hovered(String),
    /// The pointer left the region it was over.
    HoverEnd,
    #[default]
    None,
}

#[derive(Script, Widget, Animator)]
pub struct SvgSelect {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_bg: DrawSvgSelect,
    /// The drawing itself, and the parsed document behind it. Everything
    /// this widget knows about shapes it reads out of here.
    #[live]
    pub draw_svg: DrawSvg,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[apply_default]
    animator: Animator,

    /// Hold more than one region at once. A multiple picker toggles; a
    /// single one moves.
    #[live]
    pub multiple: bool,

    /// The regions held when the widget is built. It is separate from the
    /// selection itself because a live reload would otherwise put the
    /// picker back to what the file says and throw away what was chosen.
    #[live]
    pub default_selected: Vec<String>,

    #[live(0.35)]
    pub hover_weight: f64,
    #[live(0.8)]
    pub select_weight: f64,
    #[live(0.7)]
    pub disabled_weight: f64,

    #[live]
    pub hover_color: Vec4f,
    #[live]
    pub select_color: Vec4f,
    #[live]
    pub disabled_color: Vec4f,

    /// The ids of the regions found in the drawing, in the order they are
    /// first drawn.
    #[rust]
    regions: Vec<String>,
    /// Every filled shape, in draw order, with the region it belongs to.
    #[rust]
    shapes: Vec<Shape>,
    /// Every drawable node, in the order the renderer walks them.
    #[rust]
    leaves: Vec<Leaf>,
    /// The colours last written into the document, one per leaf. Comparing
    /// against it is what keeps a drawing that has not changed from being
    /// tessellated all over again.
    #[rust]
    applied: Vec<Option<[f32; 4]>>,
    /// What the document looked like when it was last read: its bounds and
    /// how many nodes it had. A drawing swapped for another one almost
    /// always differs in one or the other, and re-reading on every frame
    /// would mean flattening every curve on every frame.
    #[rust]
    read_from: Option<((f32, f32, f32, f32), usize)>,

    #[rust]
    selected: Vec<String>,
    #[rust]
    tints: Vec<(String, [f32; 4])>,
    #[rust]
    hover: Option<usize>,
    /// The region the arrow keys are on, which is lit like a hover so the
    /// keyboard can see where it is.
    #[rust]
    key_at: Option<usize>,
}

impl ScriptHook for SvgSelect {
    fn on_after_new(&mut self, _vm: &mut ScriptVm) {
        self.selected = self.default_selected.clone();
    }
}

impl SvgSelect {
    /// Read the regions out of the document, if it has arrived and is not
    /// the one already read.
    fn read_document(&mut self) {
        // The renderer draws through the viewbox transform, and so do the
        // bounds the fit is computed from, so the geometry has to be taken
        // in that same space or a press would land in the wrong place on
        // any file whose viewBox is not its own size.
        let (count, base) = {
            let Some(doc) = self.draw_svg.svg_doc.as_ref() else {
                return;
            };
            let (lw, lh) = doc.logical_size();
            let base = match doc.viewbox.as_ref() {
                Some(vb) => {
                    let (sx, sy, tx, ty) = viewbox_transform(vb, lw, lh);
                    Transform2d { a: sx, c: 0.0, e: tx, b: 0.0, d: sy, f: ty }
                }
                None => Transform2d::identity(),
            };
            (count_leaves(&doc.root), base)
        };
        let stamp = (self.draw_svg.content_bounds, count);
        if self.read_from == Some(stamp) {
            return;
        }

        let (bx0, by0, bx1, by1) = self.draw_svg.content_bounds;
        let unit = (((bx1 - bx0).max(by1 - by0)) as f64 / 100.0).max(f64::MIN_POSITIVE);

        let mut regions: Vec<String> = Vec::new();
        let mut shapes: Vec<Shape> = Vec::new();
        let mut leaves: Vec<Leaf> = Vec::new();

        let Some(mut doc) = self.draw_svg.svg_doc.take() else {
            return;
        };
        let mut index = 0usize;
        let mut take = |_at: usize, owner: Option<&str>, xf: &Transform2d, node: &mut SvgNode| {
            let region = owner.map(|id| match regions.iter().position(|r| r == id) {
                Some(at) => at,
                None => {
                    regions.push(id.to_string());
                    regions.len() - 1
                }
            });
            let style = node_style(node);
            leaves.push(Leaf { region, base: style.fill.clone() });
            if let Some(region) = region {
                let rule = style.fill_rule;
                let rings = rings_of_node(node, xf, unit);
                if let Some(bounds) = ring_bounds(&rings) {
                    shapes.push(Shape { region, rings, rule, bounds });
                }
            }
        };
        walk_leaves(&mut doc.root, None, &base, &mut index, &mut take);
        self.draw_svg.svg_doc = Some(doc);

        self.regions = regions;
        self.shapes = shapes;
        self.leaves = leaves;
        // The document is freshly parsed, so whatever was written into the
        // last one is gone with it.
        self.applied = vec![None; self.leaves.len()];
        self.read_from = Some(stamp);
        self.hover = None;
        self.key_at = None;
    }

    fn fit(&self, size: DVec2) -> Fit {
        let (x0, y0, x1, y1) = self.draw_svg.content_bounds;
        Fit {
            bounds: (x0 as f64, y0 as f64, x1 as f64, y1 as f64),
            size: (size.x, size.y),
            keep_aspect: self.draw_svg.preserve_aspect,
        }
    }

    /// The region under a point in the widget, or nothing.
    ///
    /// The shapes are tested backwards because that is the order they are
    /// painted in: what covers the point last is what the eye sees, and
    /// that is what the finger has to get.
    fn region_at(&self, abs: DVec2, rect: Rect) -> Option<usize> {
        let (x, y) = self
            .fit(rect.size)
            .to_doc(abs.x - rect.pos.x, abs.y - rect.pos.y);
        self.shapes
            .iter()
            .rev()
            .find(|shape| shape.covers(x, y))
            .map(|shape| shape.region)
    }

    fn tint_of(&self, region: usize) -> Option<[f32; 4]> {
        let id = self.regions.get(region)?;
        self.tints.iter().find(|(t, _)| t == id).map(|(_, c)| *c)
    }

    fn is_held(&self, region: usize) -> bool {
        match self.regions.get(region) {
            Some(id) => self.selected.iter().any(|s| s == id),
            None => false,
        }
    }

    /// The colour every node should be drawn in, or nothing where the file's
    /// own colour stands.
    fn wanted(&self, disabled: bool) -> Vec<Option<[f32; 4]>> {
        let mut out = Vec::with_capacity(self.leaves.len());
        for leaf in &self.leaves {
            let tint = leaf.region.and_then(|r| self.tint_of(r));
            let lit = leaf.region.is_some_and(|r| {
                !disabled && (self.hover == Some(r) || self.key_at == Some(r))
            });
            let held = leaf.region.is_some_and(|r| self.is_held(r));
            let authored = solid(&leaf.base);

            // Nothing to say about this node: leave the file alone rather
            // than writing back a colour that is already there.
            if tint.is_none() && !lit && !held && !disabled {
                out.push(None);
                continue;
            }
            // A gradient or a `none` has no colour to mix with, so a state
            // that would only have tinted it leaves it as it is.
            let Some(mut color) = tint.or(authored) else {
                out.push(None);
                continue;
            };
            if held {
                color = mix(color, rgba(self.select_color), self.select_weight as f32);
            }
            if lit {
                color = mix(color, rgba(self.hover_color), self.hover_weight as f32);
            }
            if disabled {
                color = mix(color, rgba(self.disabled_color), self.disabled_weight as f32);
            }
            out.push(Some(color));
        }
        out
    }

    /// Write the colours into the retained document, and only then tell the
    /// renderer its cached geometry is stale. Marking it stale is what costs
    /// a re-tessellation, so it is done once per change of state and never
    /// per frame.
    fn repaint(&mut self, wanted: Vec<Option<[f32; 4]>>) {
        if wanted == self.applied {
            return;
        }
        let Some(mut doc) = self.draw_svg.svg_doc.take() else {
            return;
        };
        {
            let leaves = &self.leaves;
            let base = Transform2d::identity();
            let mut index = 0usize;
            let mut put = |at: usize, _owner: Option<&str>, _xf: &Transform2d, node: &mut SvgNode| {
                let Some(leaf) = leaves.get(at) else {
                    return;
                };
                let style = node_style_mut(node);
                match wanted.get(at).copied().flatten() {
                    Some(c) => style.fill = Some(SvgPaint::Color(c[0], c[1], c[2], c[3])),
                    None => style.fill = leaf.base.clone(),
                }
            };
            walk_leaves(&mut doc.root, None, &base, &mut index, &mut put);
        }
        self.draw_svg.svg_doc = Some(doc);
        self.draw_svg.cache_valid = false;
        self.applied = wanted;
    }

    /// Take a press on a region, and say what it did.
    fn pick(&mut self, cx: &mut Cx, region: usize) {
        let Some(id) = self.regions.get(region).cloned() else {
            return;
        };
        if self.multiple {
            match self.selected.iter().position(|s| *s == id) {
                Some(at) => {
                    self.selected.remove(at);
                }
                None => self.selected.push(id.clone()),
            }
        } else {
            self.selected.clear();
            self.selected.push(id.clone());
        }
        cx.widget_action(self.uid, SvgSelectAction::Picked(id, self.selected.clone()));
        self.draw_bg.redraw(cx);
    }

    fn move_key(&mut self, cx: &mut Cx, to: usize) {
        if self.key_at != Some(to) {
            self.key_at = Some(to);
            self.draw_bg.redraw(cx);
        }
    }

    /// The ids of every region in the drawing, in draw order.
    pub fn region_ids(&self) -> Vec<String> {
        self.regions.clone()
    }

    pub fn selection(&self) -> Vec<String> {
        self.selected.clone()
    }

    pub fn is_selected(&self, id: &str) -> bool {
        self.selected.iter().any(|s| s == id)
    }

    /// The region under the pointer, while there is one.
    pub fn hovered(&self) -> Option<String> {
        self.hover.and_then(|r| self.regions.get(r).cloned())
    }

    pub fn set_selection(&mut self, cx: &mut Cx, ids: Vec<String>) {
        // A single picker holds one region however many it is handed: the
        // rest of the widget may then assume it, and a host that hands over
        // two has said something the control cannot show.
        self.selected = if self.multiple { ids } else { ids.into_iter().take(1).collect() };
        self.draw_bg.redraw(cx);
    }

    pub fn select(&mut self, cx: &mut Cx, id: &str) {
        if self.is_selected(id) {
            return;
        }
        if !self.multiple {
            self.selected.clear();
        }
        self.selected.push(id.to_string());
        self.draw_bg.redraw(cx);
    }

    pub fn deselect(&mut self, cx: &mut Cx, id: &str) {
        self.selected.retain(|s| s != id);
        self.draw_bg.redraw(cx);
    }

    pub fn clear_selection(&mut self, cx: &mut Cx) {
        if !self.selected.is_empty() {
            self.selected.clear();
            self.draw_bg.redraw(cx);
        }
    }

    /// Give one region a colour of its own, in place of the one the file
    /// was authored with. An id no region carries is kept rather than
    /// dropped: the drawing may not have arrived yet.
    pub fn set_tint(&mut self, cx: &mut Cx, id: &str, color: Vec4f) {
        let color = rgba(color);
        match self.tints.iter_mut().find(|(t, _)| t == id) {
            Some(slot) => slot.1 = color,
            None => self.tints.push((id.to_string(), color)),
        }
        self.draw_bg.redraw(cx);
    }

    pub fn clear_tint(&mut self, cx: &mut Cx, id: &str) {
        self.tints.retain(|(t, _)| t != id);
        self.draw_bg.redraw(cx);
    }

    pub fn clear_tints(&mut self, cx: &mut Cx) {
        if !self.tints.is_empty() {
            self.tints.clear();
            self.draw_bg.redraw(cx);
        }
    }
}

/// How many drawable nodes a tree has, without touching their geometry.
fn count_leaves(nodes: &[SvgNode]) -> usize {
    let mut count = 0;
    for node in nodes {
        match node {
            SvgNode::Group(group) => count += count_leaves(&group.children),
            _ => count += 1,
        }
    }
    count
}

impl Widget for SvgSelect {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.animator_toggle(cx, disabled, Animate::Yes, ids!(disabled.on), ids!(disabled.off));
    }

    fn disabled(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(disabled.on))
    }

    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(select) || method == live_id!(deselect) {
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                let value = vm.bx.heap.vec_value(args_obj, 0, trap);
                if !value.is_err() {
                    if let Some(id) = vm.bx.heap.cast_to_owned_string(value, "picking a region") {
                        vm.with_cx_mut(|cx| {
                            if method == live_id!(select) {
                                self.select(cx, &id);
                            } else {
                                self.deselect(cx, &id);
                            }
                        });
                    }
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(clear) {
            vm.with_cx_mut(|cx| self.clear_selection(cx));
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(selection) {
            let text = self.selected.join(" ");
            let str_val = vm.bx.heap.new_string_from_str(&text);
            return ScriptAsyncResult::Return(str_val.into());
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.animator_handle_event(cx, event);
        if self.animator_in_state(cx, ids!(disabled.on)) {
            return;
        }
        let uid = self.uid;

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Default);
            }
            Hit::FingerHoverOver(fe) => {
                let at = self.region_at(fe.abs, fe.rect);
                if self.hover != at {
                    self.hover = at;
                    match at.and_then(|r| self.regions.get(r).cloned()) {
                        Some(id) => cx.widget_action(uid, SvgSelectAction::Hovered(id)),
                        None => cx.widget_action(uid, SvgSelectAction::HoverEnd),
                    }
                    self.draw_bg.redraw(cx);
                }
                // The cursor is the only thing that says a picture is a
                // control, and it has to say so per region: most of a map
                // is sea.
                cx.set_cursor(if at.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    cx.widget_action(uid, SvgSelectAction::HoverEnd);
                    self.draw_bg.redraw(cx);
                }
                cx.set_cursor(MouseCursor::Default);
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                // A press that landed on nothing is a miss, and a miss
                // leaves the answer alone. On a picture a miss is ordinary:
                // most of the room between the parts belongs to no part.
                if let Some(region) = self.region_at(fe.abs, fe.rect) {
                    self.key_at = Some(region);
                    self.pick(cx, region);
                }
            }
            Hit::KeyFocus(_) => {
                self.animator_play(cx, ids!(focus.on));
            }
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, ids!(focus.off));
                if self.key_at.take().is_some() {
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::KeyDown(ke) => {
                if self.regions.is_empty() {
                    return;
                }
                let last = self.regions.len() - 1;
                match ke.key_code {
                    // Without a pointer there is no region under anything,
                    // so the arrows walk the drawing in the order it is
                    // painted and light where they are.
                    KeyCode::ArrowRight | KeyCode::ArrowDown => {
                        let to = match self.key_at {
                            Some(at) if at < last => at + 1,
                            Some(_) => 0,
                            None => 0,
                        };
                        self.move_key(cx, to);
                    }
                    KeyCode::ArrowLeft | KeyCode::ArrowUp => {
                        let to = match self.key_at {
                            Some(0) | None => last,
                            Some(at) => at - 1,
                        };
                        self.move_key(cx, to);
                    }
                    KeyCode::Home => self.move_key(cx, 0),
                    KeyCode::End => self.move_key(cx, last),
                    KeyCode::ReturnKey | KeyCode::NumpadEnter | KeyCode::Space => {
                        if let Some(at) = self.key_at {
                            self.pick(cx, at);
                        }
                    }
                    _ => (),
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // Ask for the document rather than wait for it: a resource may still
        // be on its way, in which case there is nothing to read and the next
        // draw tries again.
        let _ = self.draw_svg.measure(cx, walk);
        self.read_document();
        // A document that has just been parsed carries the colours the FILE
        // has, whatever was last written into the one before it. The parse
        // is the only thing that drops the geometry cache, so this is where
        // it shows: forget the record, and the repaint below puts the state
        // back rather than trusting it.
        if !self.draw_svg.cache_valid && !self.applied.iter().all(Option::is_none) {
            self.applied = vec![None; self.leaves.len()];
        }

        // The ring is drawn from the box the walk is ABOUT to take, because
        // an instance's values are read when it is added and the rect is
        // only known once it has been. The two agree in every ordinary case;
        // where they could not, the cost is a ring a little out of place and
        // never a press in the wrong region, which reads the real rect.
        let peek = cx.peek_walk_turtle(walk);
        let (ax, ay, aw, ah) = self.fit(peek.size).art();
        self.draw_bg.art_pos = dvec2(ax, ay).into();
        self.draw_bg.art_size = dvec2(aw, ah).into();

        let disabled = self.animator_in_state(cx, ids!(disabled.on));
        let wanted = self.wanted(disabled);
        self.repaint(wanted);

        let rect = self.draw_bg.draw_walk(cx, walk);
        self.draw_svg.draw_abs(cx, rect);

        if !disabled {
            cx.add_nav_stop(self.draw_bg.area(), NavRole::DropDown, Inset::default());
        }
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.selected.join(", ")
    }
}

impl SvgSelectRef {
    /// The ids of every region the drawing offers, in draw order. Empty
    /// until the drawing has been loaded and drawn once.
    pub fn region_ids(&self) -> Vec<String> {
        self.borrow().map(|inner| inner.region_ids()).unwrap_or_default()
    }

    pub fn selection(&self) -> Vec<String> {
        self.borrow().map(|inner| inner.selection()).unwrap_or_default()
    }

    pub fn is_selected(&self, id: &str) -> bool {
        self.borrow().map(|inner| inner.is_selected(id)).unwrap_or(false)
    }

    pub fn hovered(&self) -> Option<String> {
        self.borrow().and_then(|inner| inner.hovered())
    }

    pub fn set_selection(&self, cx: &mut Cx, ids: Vec<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_selection(cx, ids);
        }
    }

    pub fn select(&self, cx: &mut Cx, id: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.select(cx, id);
        }
    }

    pub fn deselect(&self, cx: &mut Cx, id: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.deselect(cx, id);
        }
    }

    pub fn clear_selection(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear_selection(cx);
        }
    }

    /// Give one region a colour of its own. What a map showing a quantity
    /// per region is for.
    pub fn set_tint(&self, cx: &mut Cx, id: &str, color: Vec4f) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_tint(cx, id, color);
        }
    }

    pub fn clear_tint(&self, cx: &mut Cx, id: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear_tint(cx, id);
        }
    }

    pub fn clear_tints(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear_tints(cx);
        }
    }

    /// The region a press landed on.
    pub fn picked(&self, actions: &Actions) -> Option<String> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            SvgSelectAction::Picked(id, _) => Some(id),
            _ => None,
        }
    }

    /// The whole selection, after a press changed it.
    pub fn changed(&self, actions: &Actions) -> Option<Vec<String>> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            SvgSelectAction::Picked(_, selection) => Some(selection),
            _ => None,
        }
    }

    /// The region the pointer moved onto.
    pub fn hovered_action(&self, actions: &Actions) -> Option<String> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            SvgSelectAction::Hovered(id) => Some(id),
            _ => None,
        }
    }

    /// The pointer left the region it was over.
    pub fn hover_ended(&self, actions: &Actions) -> bool {
        actions
            .find_widget_action(self.widget_uid())
            .map(|item| matches!(item.cast(), SvgSelectAction::HoverEnd))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(pts: &[(f64, f64)]) -> Ring {
        pts.to_vec()
    }

    /// A square from (0,0) to (10,10), wound clockwise in screen terms.
    fn square() -> Ring {
        ring(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)])
    }

    #[test]
    fn a_point_inside_a_simple_shape_is_inside_it() {
        let rings = vec![square()];
        assert!(covers(&rings, FillRule::NonZero, 5.0, 5.0));
        assert!(covers(&rings, FillRule::EvenOdd, 5.0, 5.0));
        assert!(covers(&rings, FillRule::NonZero, 0.5, 9.5), "and so is one near a corner");
    }

    #[test]
    fn a_point_outside_it_is_outside_it() {
        let rings = vec![square()];
        assert!(!covers(&rings, FillRule::NonZero, -1.0, 5.0));
        assert!(!covers(&rings, FillRule::NonZero, 11.0, 5.0));
        assert!(!covers(&rings, FillRule::NonZero, 5.0, -1.0));
        assert!(!covers(&rings, FillRule::NonZero, 5.0, 11.0));
        // Level with the shape but past its right edge: the ray still has
        // to miss, which is the case the crossing count gets wrong when the
        // straddle test is not half-open.
        assert!(!covers(&rings, FillRule::EvenOdd, 20.0, 0.0));
    }

    #[test]
    fn a_hole_is_not_part_of_the_shape() {
        // The inner ring runs the other way about, so the winding numbers
        // cancel inside it; even-odd needs no direction at all.
        let inner_reversed = ring(&[(3.0, 3.0), (3.0, 7.0), (7.0, 7.0), (7.0, 3.0)]);
        let rings = vec![square(), inner_reversed];
        assert!(!covers(&rings, FillRule::NonZero, 5.0, 5.0), "the hole is empty");
        assert!(covers(&rings, FillRule::NonZero, 1.0, 5.0), "the ring round it is not");
        assert!(!covers(&rings, FillRule::EvenOdd, 5.0, 5.0));
        assert!(covers(&rings, FillRule::EvenOdd, 1.0, 5.0));
    }

    #[test]
    fn a_hole_wound_the_same_way_is_a_hole_only_under_even_odd() {
        // This is the whole of the difference between the two rules, and
        // the reason the shape's own rule is carried rather than assumed.
        let inner_same = ring(&[(3.0, 3.0), (7.0, 3.0), (7.0, 7.0), (3.0, 7.0)]);
        let rings = vec![square(), inner_same];
        assert!(covers(&rings, FillRule::NonZero, 5.0, 5.0), "two turns the same way fill");
        assert!(!covers(&rings, FillRule::EvenOdd, 5.0, 5.0), "two crossings do not");
    }

    #[test]
    fn the_notch_of_a_concave_shape_is_outside_it() {
        // A C: a square with a bite taken out of its right side.
        let c = ring(&[
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 3.0),
            (4.0, 3.0),
            (4.0, 7.0),
            (10.0, 7.0),
            (10.0, 10.0),
            (0.0, 10.0),
        ]);
        let rings = vec![c];
        assert!(covers(&rings, FillRule::NonZero, 2.0, 5.0), "the spine is inside");
        assert!(covers(&rings, FillRule::NonZero, 7.0, 1.5), "and so is the top arm");
        assert!(!covers(&rings, FillRule::NonZero, 7.0, 5.0), "the bite is not");
        assert!(!covers(&rings, FillRule::EvenOdd, 7.0, 5.0));
    }

    #[test]
    fn a_point_level_with_a_corner_stays_in_its_own_shape() {
        // A diamond, whose left and right corners sit exactly on the ray
        // cast from a point at the same height. Counted twice or not at
        // all, the middle of the shape falls out of it.
        let diamond = ring(&[(5.0, 0.0), (10.0, 5.0), (5.0, 10.0), (0.0, 5.0)]);
        let rings = vec![diamond];
        assert!(covers(&rings, FillRule::NonZero, 5.0, 5.0));
        assert!(covers(&rings, FillRule::EvenOdd, 5.0, 5.0));
        assert!(!covers(&rings, FillRule::EvenOdd, 12.0, 5.0));
    }

    #[test]
    fn an_empty_path_covers_nothing() {
        let none: Vec<Ring> = Vec::new();
        assert!(!covers(&none, FillRule::NonZero, 0.0, 0.0));
        assert!(!covers(&none, FillRule::EvenOdd, 5.0, 5.0));
        // And neither does a path that never bounded an area: two points
        // are a line, and a line has no inside.
        let line = vec![ring(&[(0.0, 0.0), (10.0, 10.0)])];
        assert!(!covers(&line, FillRule::NonZero, 5.0, 5.0));
        assert!(!covers(&line, FillRule::EvenOdd, 5.0, 5.0));
        assert!(ring_bounds(&none).is_none());
    }

    #[test]
    fn a_drawing_that_keeps_its_aspect_is_centred_in_the_room_it_has() {
        // A square drawing in a wide box: the same scale on both axes, and
        // the slack shared between left and right.
        let fit = Fit { bounds: (0.0, 0.0, 100.0, 100.0), size: (400.0, 200.0), keep_aspect: true };
        assert_eq!(fit.scale(), (2.0, 2.0));
        assert_eq!(fit.art(), (100.0, 0.0, 200.0, 200.0));
    }

    #[test]
    fn a_press_lands_where_the_drawing_was_put() {
        // The corners of the drawn box map to the corners of the drawing,
        // which is the whole contract between the picture and the finger.
        let fit = Fit { bounds: (0.0, 0.0, 100.0, 100.0), size: (400.0, 200.0), keep_aspect: true };
        assert_eq!(fit.to_doc(100.0, 0.0), (0.0, 0.0));
        assert_eq!(fit.to_doc(300.0, 200.0), (100.0, 100.0));
        assert_eq!(fit.to_doc(200.0, 100.0), (50.0, 50.0));
        // The letterbox either side is off the drawing altogether.
        let (x, _) = fit.to_doc(10.0, 100.0);
        assert!(x < 0.0);
    }

    #[test]
    fn a_viewbox_that_does_not_start_at_the_origin_is_carried_through() {
        let fit = Fit { bounds: (-50.0, 10.0, 50.0, 110.0), size: (200.0, 200.0), keep_aspect: true };
        assert_eq!(fit.to_doc(0.0, 0.0), (-50.0, 10.0));
        assert_eq!(fit.to_doc(200.0, 200.0), (50.0, 110.0));
    }

    #[test]
    fn a_drawing_told_not_to_keep_its_aspect_fills_the_box() {
        let fit = Fit { bounds: (0.0, 0.0, 100.0, 50.0), size: (400.0, 400.0), keep_aspect: false };
        assert_eq!(fit.scale(), (4.0, 8.0));
        assert_eq!(fit.art(), (0.0, 0.0, 400.0, 400.0));
        assert_eq!(fit.to_doc(400.0, 400.0), (100.0, 50.0));
    }

    #[test]
    fn a_drawing_with_no_extent_answers_rather_than_dividing_by_it() {
        // An empty document, or one whose every shape is a point.
        let fit = Fit { bounds: (5.0, 5.0, 5.0, 5.0), size: (200.0, 100.0), keep_aspect: true };
        assert_eq!(fit.scale(), (1.0, 1.0));
        assert_eq!(fit.art(), (0.0, 0.0, 200.0, 100.0));
        assert_eq!(fit.to_doc(3.0, 4.0), (3.0, 4.0));
    }

    #[test]
    fn a_curve_is_flattened_into_the_ring_it_traces() {
        // A quarter turn drawn as one cubic: the ends are exact and the
        // middle bulges out toward the control points.
        let mut path = VectorPath::new();
        path.move_to(0.0, 0.0);
        path.bezier_to(10.0, 0.0, 10.0, 0.0, 10.0, 10.0);
        path.line_to(0.0, 10.0);
        path.close();
        let rings = rings_of_path(&path, &Transform2d::identity(), 1.0);
        assert_eq!(rings.len(), 1);
        assert!(rings[0].len() > 4, "the curve became several segments");
        assert!(covers(&rings, FillRule::NonZero, 5.0, 5.0));
        // The curve leans on the top-right corner, so the corner itself is
        // outside the shape: a box around the path would have said inside.
        assert!(!covers(&rings, FillRule::NonZero, 9.8, 1.0));
    }

    #[test]
    fn a_shape_that_was_never_closed_still_has_an_inside() {
        // A renderer fills an open subpath as though it were closed, so the
        // hit test has to agree with it.
        let mut path = VectorPath::new();
        path.move_to(0.0, 0.0);
        path.line_to(10.0, 0.0);
        path.line_to(10.0, 10.0);
        path.line_to(0.0, 10.0);
        let rings = rings_of_path(&path, &Transform2d::identity(), 1.0);
        assert_eq!(rings.len(), 1);
        assert!(covers(&rings, FillRule::NonZero, 5.0, 5.0));
    }

    #[test]
    fn a_transform_moves_the_geometry_the_hit_test_uses() {
        let mut path = VectorPath::new();
        path.rect(0.0, 0.0, 10.0, 10.0);
        let xf = Transform2d::scale(2.0, 2.0).then(&Transform2d::translate(100.0, 0.0));
        let rings = rings_of_path(&path, &xf, 1.0);
        assert!(covers(&rings, FillRule::NonZero, 110.0, 10.0));
        assert!(!covers(&rings, FillRule::NonZero, 5.0, 5.0), "not where it was authored");
    }

    #[test]
    fn a_shape_is_rejected_by_its_box_before_its_edges_are_walked() {
        let shape = Shape {
            region: 0,
            rings: vec![square()],
            rule: FillRule::NonZero,
            bounds: ring_bounds(&[square()]).unwrap(),
        };
        assert_eq!(shape.bounds, (0.0, 0.0, 10.0, 10.0));
        assert!(shape.covers(5.0, 5.0));
        assert!(!shape.covers(50.0, 5.0));
    }

    #[test]
    fn mixing_moves_all_the_way_and_no_further() {
        let a = [0.0, 0.0, 0.0, 1.0];
        let b = [1.0, 1.0, 1.0, 1.0];
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
        assert_eq!(mix(a, b, 0.5), [0.5, 0.5, 0.5, 1.0]);
        assert_eq!(mix(a, b, 4.0), b, "a weight past the end is the end");
    }

    #[test]
    fn only_a_plain_colour_can_be_mixed_with() {
        assert_eq!(solid(&Some(SvgPaint::Color(0.5, 0.25, 0.0, 1.0))), Some([0.5, 0.25, 0.0, 1.0]));
        assert_eq!(solid(&None), None);
        assert_eq!(solid(&Some(SvgPaint::None)), None);
        assert_eq!(solid(&Some(SvgPaint::GradientRef("g".to_string()))), None);
    }
}
