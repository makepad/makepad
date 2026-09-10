//! CornerCapView — rounded corners for a surface whose shader the library
//! does not own.
//!
//! **It is a fake, and it is worth saying so first.** Nothing here rounds
//! anything. The content is drawn square and four small patches of the
//! surrounding chrome are painted over its corners afterwards, so what the
//! eye reads as a curve is really the chrome colour arriving early. Over a
//! flat ground of exactly that colour the illusion is total. Over a
//! gradient, a picture, a shadow or a glass panel it is not: each corner
//! shows as a slightly wrong square, and no setting on this widget fixes
//! that, because there is no setting that could. `CachedRoundedView` is
//! the version that is correct in every case — it draws its children into
//! a texture and samples that inside a real rounded path — and it is what
//! to reach for unless the paragraph below applies.
//!
//! # Why it exists anyway
//!
//! `CachedRoundedView` costs an offscreen render target the size of its
//! rect, re-rendered whenever anything inside it changes, plus an SDF test
//! over every pixel on the way back. For a panel of buttons that is
//! nothing: the target is built once and reused for as long as the panel
//! sits still. For content that changes every frame and whose shader the
//! library cannot edit — video, a chart being fed live, a page of a
//! document, a map canvas, an embedded browser — it is the wrong trade
//! twice, because the target is rebuilt on every one of those frames and
//! the full-rect test runs on every one of them too. Four patches the size
//! of the radius run their per-pixel arithmetic over a few hundred pixels
//! instead, and the expensive surface goes on drawing the same plain quad
//! it always drew.
//!
//! # What it will not do
//!
//! It does not clip. A child that paints outside the rounded corner still
//! paints there; it is simply covered up afterwards. Nothing is prevented,
//! so a corner is only ever as clean as the chrome colour is right.
//!
//! It cannot find out what is behind it. `draw_cap.cap_color` is a value
//! somebody has to keep true. It defaults to the page ground, which is
//! right while the surface sits on the page and wrong the moment it is
//! moved into a card, a well or a coloured band — and the widget will not
//! notice, because a widget cannot see the pixels under it.
//!
//! It claims no hits and reads no events of its own: the patches are draw
//! calls, and everything a pointer does goes to the children as though the
//! caps were not there. A press that lands in the fake corner still reaches
//! the content under it — and so does a press in a `CachedRoundedView`'s
//! corner, whose rounding lives entirely in a fragment shader that hit
//! testing never runs. Neither of them clips a hit; the fake is not better
//! here, it is the same.
//!
//! # Padding
//!
//! `padding` written on one of these is forwarded to the children, and the
//! caps follow the padding in: a padded CornerCapView rounds the box its
//! children were given, not the empty border around it. Capping the outer
//! box would round nothing but air and leave the surface square inside.
//! Only the padding is followed — a child's own `margin`, or a child left
//! smaller than that box by `align`, is not — and a background the padded
//! view paints itself fills the outer box, so its corners stay square.
use crate::{makepad_derive_widget::*, makepad_draw::*, view::View, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // One corner patch: the chrome colour everywhere OUTSIDE a quarter
    // circle and nothing at all inside it, so four of them laid on the
    // corners of a rectangle leave a rounded shape behind. `length` to the
    // arc centre is an exact distance, so clamping it across one pixel is
    // the whole antialiasing — one DEVICE pixel, measured the way
    // `Sdf2d.antialias` measures it, because the point this widget has to
    // survive is standing next to a real rounded corner: a band a layout
    // point wide would be dpi times softer than that corner and the two
    // would come apart the moment the window moved to a denser display.
    // Premultiplied by hand for the same reason the chart marks are: this
    // builds its colour rather than filling a shape, so nothing else will.
    set_type_default() do #(DrawCornerPatch::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let p = self.pos * self.rect_size
            // The same anti-alias scale Sdf2d uses: the band coverage
            // falls off across is one layout point's diagonal, about 1.4
            // device pixels, whatever the dpi.
            let aa = 1.0 / length(vec2(length(dFdx(p)), length(dFdy(p))))
            let d = length(p - self.center) - self.arc_r
            let cover = clamp(d * aa + 0.5, 0.0, 1.0)
            return Pal.premul(vec4(self.cap_color.xyz, self.cap_color.w * cover))
        }
    }

    mod.widgets.CornerCapViewBase = #(CornerCapView::register_widget(vm))

    /** Rounds the corners of content the library cannot round, by painting
     * the chrome behind it back over them.
     *
     * A fake: over a flat ground it is exact, over a gradient or a picture
     * each corner is a slightly wrong square. `CachedRoundedView` is the
     * one that is right everywhere and costs a render target to be. Reach
     * for this only when the content redraws every frame. */
    mod.widgets.CornerCapView = set_type_default() do mod.widgets.CornerCapViewBase{
        width: Fill
        height: Fill
        // The expected child count is one — a surface — with the odd label
        // or spinner over it, which is what Overlay is for.
        flow: Overlay

        /** the corner radius every corner follows unless given its own 0..40 step 0.5 */
        radius: theme.radius_l
        /** top-left corner; -1 follows radius -1..40 step 0.5 */
        radius_tl: -1.0
        /** top-right corner; -1 follows radius -1..40 step 0.5 */
        radius_tr: -1.0
        /** bottom-right corner; -1 follows radius -1..40 step 0.5 */
        radius_br: -1.0
        /** bottom-left corner; -1 follows radius -1..40 step 0.5 */
        radius_bl: -1.0

        draw_cap +: {
            /** the chrome painted into the corners: whatever is really behind the content */
            cap_color: theme.color_bg_app
        }
    }
}

/// One corner patch. `center` is patch-local, and the patch paints its
/// colour outside `arc_r` of it — so putting the centre on the patch's
/// inner corner leaves the outer corner filled and everything else clear.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCornerPatch {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub center: Vec2f,
    #[live]
    pub arc_r: f32,
    #[live]
    pub cap_color: Vec4f,
}

/// A corner, in the order the library writes them everywhere else.
const TL: usize = 0;
const TR: usize = 1;
const BR: usize = 2;
const BL: usize = 3;

/// Under half a point there is no curve left to draw: at any dpi the patch
/// is a sliver of chrome on a corner that already reads as square, and it
/// would still cost a draw call.
const MIN_CAP: f64 = 0.5;

/// Where one corner's patch goes and where its arc sits inside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CapPatch {
    /// The patch's own rect, in the same space as the capped rect.
    pub rect: Rect,
    /// The arc centre, relative to the patch's top-left.
    pub center: DVec2,
    pub radius: f64,
}

/// The four radii the patches will use, in TL, TR, BR, BL order.
///
/// A corner left negative follows the shared `radius`, which is how every
/// other radius in the library spells "I did not ask for anything special
/// here" — a card squares its bottom two and leaves the rest alone. Each
/// one is then held to half the shorter side, so two caps on one edge can
/// meet but never overlap, and a rect with no area gets no caps at all
/// rather than caps larger than the thing they are capping.
pub fn cap_radii(radius: f64, corners: [f64; 4], size: DVec2) -> [f64; 4] {
    // Written as a positive test so a size that is negative or NaN — a rect
    // read back before anything was laid out — falls out as no caps rather
    // than through `min`, which answers a NaN limit with the radius.
    let limit = size.x.min(size.y) * 0.5;
    if !(limit > 0.0) {
        return [0.0; 4];
    }
    let mut out = [0.0; 4];
    for (slot, corner) in out.iter_mut().zip(corners) {
        let r = if corner < 0.0 { radius } else { corner };
        // And a positive test again, so a radius that is negative or NaN —
        // whether it was written on the corner or inherited from the shared
        // one — is no cap, rather than a NaN carried into the quad or a
        // patch with a backwards arc in it. Guarding `radius` separately
        // before the fallback would be the same test twice: a negative
        // shared radius can only reach a slot through this one.
        *slot = if r > 0.0 { r.min(limit) } else { 0.0 };
    }
    out
}

/// The box the caps go on: the children's box, not the widget's own.
///
/// `padding` on a CornerCapView is forwarded to the inner view, which insets
/// its children by it. Capping the outer box would then cut the corners off
/// a border of empty air and leave the surface's own corners square a
/// padding's width inside — a silent nothing, from a container whose whole
/// job is rounding that surface. So the caps follow the children in.
pub fn capped_rect(rect: Rect, padding: Inset) -> Rect {
    Rect {
        pos: dvec2(rect.pos.x + padding.left, rect.pos.y + padding.top),
        // Padding wider than the box leaves no box at all, not a negative
        // one: `cap_radii` reads the size and a rect turned inside out
        // would be capped as though it were a large one.
        size: dvec2(
            (rect.size.x - padding.left - padding.right).max(0.0),
            (rect.size.y - padding.top - padding.bottom).max(0.0),
        ),
    }
}

/// The patch for one corner of `rect`, or nothing when the radius is too
/// small to be seen.
pub fn cap_patch(rect: Rect, corner: usize, radius: f64) -> Option<CapPatch> {
    if radius < MIN_CAP {
        return None;
    }
    let far = dvec2(rect.pos.x + rect.size.x - radius, rect.pos.y + rect.size.y - radius);
    // The arc centre is the patch's INNER corner — the one pointing at the
    // middle of the capped rect — so the quarter circle it cuts opens
    // towards the content and the chrome is left on the outside.
    let (pos, center) = match corner {
        TL => (rect.pos, dvec2(radius, radius)),
        TR => (dvec2(far.x, rect.pos.y), dvec2(0.0, radius)),
        BR => (far, dvec2(0.0, 0.0)),
        BL => (dvec2(rect.pos.x, far.y), dvec2(radius, 0.0)),
        _ => return None,
    };
    Some(CapPatch { rect: Rect { pos, size: dvec2(radius, radius) }, center, radius })
}

#[derive(Clone)]
enum DrawState {
    Content,
    Caps,
}

/// A container that draws its children and then paints its own corners
/// over them. See the module docs: it is a fake, and `CachedRoundedView`
/// is the honest one.
#[derive(Script, ScriptHook, Widget)]
pub struct CornerCapView {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,

    #[live]
    draw_cap: DrawCornerPatch,

    /// The radius every corner takes unless it was given one of its own.
    #[live(8.0)]
    pub radius: f64,
    #[live(-1.0)]
    pub radius_tl: f64,
    #[live(-1.0)]
    pub radius_tr: f64,
    #[live(-1.0)]
    pub radius_br: f64,
    #[live(-1.0)]
    pub radius_bl: f64,

    #[rust]
    draw_state: DrawStateWrap<DrawState>,
    /// The whole walk, children and caps, captured when our own turtle ends.
    /// Marked `#[area]` so `Widget::area()` reports it: without that the
    /// derive routes `area()` to the `#[deref]` view and this field is
    /// written every frame and read by nothing.
    #[rust]
    #[area]
    area: Area,
}

impl Widget for CornerCapView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // The children and the caps go inside ONE turtle of our own so that
        // the parent sees a single walk. A quad drawn after a child's walk
        // has finished is not carried along when the parent aligns or
        // distributes that walk, and the caps would then sit a few points
        // off the content they are capping.
        if self.draw_state.begin(cx, DrawState::Content) {
            if !self.view.visible() {
                self.draw_state.end();
                return DrawStep::done();
            }
            cx.begin_turtle(walk, Layout::default());
        }

        if let Some(DrawState::Content) = self.draw_state.get() {
            // The inner view fills the turtle, or measures it when the walk
            // asked us to fit — a Fill inside a Fit collapses to nothing,
            // and the content would be laid out and never painted.
            let inner = Walk {
                width: if walk.width.is_fit() { Size::fit() } else { Size::fill() },
                height: if walk.height.is_fit() { Size::fit() } else { Size::fill() },
                ..Walk::default()
            };
            self.view.draw_walk(cx, scope, inner)?;
            self.draw_state.set(DrawState::Caps);
        }

        if let Some(DrawState::Caps) = self.draw_state.get() {
            // The children's box, not ours: padding rides on the inner view
            // and moves the children in, and caps left on the outer corners
            // would round a border of air and leave the surface square.
            let rect = capped_rect(self.view.area().rect(cx), self.view.layout.padding);
            self.draw_caps(cx, rect);
            cx.end_turtle_with_area(&mut self.area);
            self.draw_state.end();
        }

        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // The caps are paint and nothing else: every event goes straight to
        // the children, including one that lands in a corner the caps have
        // covered up.
        self.view.handle_event(cx, event, scope);
    }
}

impl CornerCapView {
    fn draw_caps(&mut self, cx: &mut Cx2d, rect: Rect) {
        let radii = cap_radii(
            self.radius,
            [self.radius_tl, self.radius_tr, self.radius_br, self.radius_bl],
            rect.size,
        );
        for (corner, radius) in radii.iter().copied().enumerate() {
            let Some(patch) = cap_patch(rect, corner, radius) else {
                continue;
            };
            self.draw_cap.arc_r = patch.radius as f32;
            self.draw_cap.center = vec2f(patch.center.x as f32, patch.center.y as f32);
            self.draw_cap.draw_abs(cx, patch.rect);
        }
    }

    /// The shared radius, and a redraw, because the caps are painted from
    /// it rather than animated to it.
    pub fn set_radius(&mut self, cx: &mut Cx, radius: f64) {
        if self.radius != radius {
            self.radius = radius;
            self.view.redraw(cx);
        }
    }

    /// The four corners as they stand, in TL, TR, BR, BL order, `-1` where
    /// the corner is following the shared radius. Handed out whole because
    /// that is what it takes to change one of them and leave the rest.
    pub fn corner_radii(&self) -> [f64; 4] {
        [self.radius_tl, self.radius_tr, self.radius_br, self.radius_bl]
    }

    /// The four corners, and a redraw. `-1` in a slot puts that corner back
    /// on the shared `radius`, which is what the DSL spells the same way.
    pub fn set_corner_radii(&mut self, cx: &mut Cx, radii: [f64; 4]) {
        if self.corner_radii() != radii {
            self.radius_tl = radii[TL];
            self.radius_tr = radii[TR];
            self.radius_br = radii[BR];
            self.radius_bl = radii[BL];
            self.view.redraw(cx);
        }
    }

    /// The chrome the corners are painted with. The caller is asserting
    /// what is behind the surface; nothing checks it.
    pub fn set_cap_color(&mut self, cx: &mut Cx, color: Vec4f) {
        if self.draw_cap.cap_color != color {
            self.draw_cap.cap_color = color;
            self.view.redraw(cx);
        }
    }
}

impl CornerCapViewRef {
    pub fn set_radius(&self, cx: &mut Cx, radius: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_radius(cx, radius);
        }
    }

    /// All four following the shared radius when the ref points at nothing,
    /// which is what a CornerCapView that has never been touched says too.
    pub fn corner_radii(&self) -> [f64; 4] {
        self.borrow().map(|inner| inner.corner_radii()).unwrap_or([-1.0; 4])
    }

    pub fn set_corner_radii(&self, cx: &mut Cx, radii: [f64; 4]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_corner_radii(cx, radii);
        }
    }

    pub fn set_cap_color(&self, cx: &mut Cx, color: Vec4f) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_cap_color(cx, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rect big enough that nothing in these tests hits the size limit
    /// unless the test is about the size limit.
    fn wide() -> Rect {
        Rect { pos: dvec2(10.0, 20.0), size: dvec2(200.0, 100.0) }
    }

    #[test]
    fn a_corner_left_negative_follows_the_shared_radius() {
        let r = cap_radii(6.0, [-1.0, -1.0, -1.0, -1.0], wide().size);
        assert_eq!(r, [6.0; 4]);
    }

    #[test]
    fn a_corner_given_a_radius_of_its_own_keeps_it() {
        // What a card asks for: the top two rounded, the bottom two square
        // because the body of the card is directly under them.
        let r = cap_radii(8.0, [-1.0, -1.0, 0.0, 0.0], wide().size);
        assert_eq!(r, [8.0, 8.0, 0.0, 0.0]);
    }

    #[test]
    fn no_corner_may_grow_past_half_the_shorter_side() {
        // Two caps on one edge may meet in the middle; they may not pass
        // through each other, and none may be larger than the rect.
        let r = cap_radii(999.0, [-1.0, -1.0, -1.0, -1.0], dvec2(200.0, 100.0));
        assert_eq!(r, [50.0; 4]);
        // The SHORT side is where the limit bites: two caps summing to 200
        // fit across the 200-wide top and prove nothing, and would still
        // fit if the half were dropped.
        assert!(r[TL] + r[BL] <= 100.0, "the two left caps still fit down the short side");
    }

    #[test]
    fn a_rect_with_no_area_is_capped_with_nothing() {
        assert_eq!(cap_radii(8.0, [-1.0; 4], dvec2(0.0, 100.0)), [0.0; 4]);
        assert_eq!(cap_radii(8.0, [-1.0; 4], dvec2(200.0, -3.0)), [0.0; 4]);
        // `min` ignores a NaN on one side, so only a size that is NaN
        // through and through arrives as a NaN limit — and `min` ignores
        // that one too, so without the positive test on the limit the caps
        // come out at the full radius on a rect whose size nobody knows.
        assert_eq!(cap_radii(8.0, [-1.0; 4], dvec2(f64::NAN, f64::NAN)), [0.0; 4]);
    }

    #[test]
    fn a_radius_that_is_negative_or_not_a_number_reads_as_no_rounding() {
        // Each of these is `r.min(limit)` away from a cap that is wrong
        // rather than absent: -4 stays -4, and NaN comes back out of `min`
        // as the limit — a corner nobody asked for, rounded to the largest
        // radius the box allows.
        assert_eq!(cap_radii(-4.0, [-1.0; 4], wide().size), [0.0; 4]);
        assert_eq!(cap_radii(f64::NAN, [-1.0; 4], wide().size), [0.0; 4]);
        assert_eq!(cap_radii(8.0, [f64::NAN, 4.0, 4.0, 4.0], wide().size), [0.0, 4.0, 4.0, 4.0]);
        // And a corner with a number of its own is not dragged down with a
        // shared radius it never asked to follow.
        assert_eq!(cap_radii(-4.0, [-1.0, 6.0, -1.0, 6.0], wide().size), [0.0, 6.0, 0.0, 6.0]);
    }

    #[test]
    fn padding_moves_the_caps_in_to_where_the_children_actually_are() {
        // Padding rides on the inner view, so the children sit inside it and
        // caps left on the outer corners would round a border of air.
        let r = capped_rect(wide(), Inset { left: 10.0, top: 4.0, right: 10.0, bottom: 6.0 });
        assert_eq!(r.pos, dvec2(20.0, 24.0), "in by the left and top padding");
        assert_eq!(r.size, dvec2(180.0, 90.0), "and short by both sides of it");
    }

    #[test]
    fn a_box_padded_past_its_own_width_is_capped_with_nothing() {
        let r = capped_rect(wide(), Inset { left: 150.0, top: 0.0, right: 150.0, bottom: 0.0 });
        assert_eq!(r.size.x, 0.0, "the children's box collapses rather than turning inside out");
        assert_eq!(cap_radii(8.0, [-1.0; 4], r.size), [0.0; 4]);
    }

    #[test]
    fn the_top_left_patch_covers_the_corner_with_its_arc_centre_pointing_inwards() {
        let patch = cap_patch(wide(), TL, 12.0).unwrap();
        assert_eq!(patch.rect.pos, dvec2(10.0, 20.0), "it starts where the rect starts");
        assert_eq!(patch.rect.size, dvec2(12.0, 12.0), "and is exactly the radius across");
        assert_eq!(patch.center, dvec2(12.0, 12.0), "the arc opens towards the content");
    }

    #[test]
    fn the_other_three_patches_sit_on_their_own_corners() {
        let rect = wide();
        let tr = cap_patch(rect, TR, 12.0).unwrap();
        assert_eq!(tr.rect.pos, dvec2(198.0, 20.0));
        assert_eq!(tr.center, dvec2(0.0, 12.0));

        let br = cap_patch(rect, BR, 12.0).unwrap();
        assert_eq!(br.rect.pos, dvec2(198.0, 108.0));
        assert_eq!(br.center, dvec2(0.0, 0.0));

        let bl = cap_patch(rect, BL, 12.0).unwrap();
        assert_eq!(bl.rect.pos, dvec2(10.0, 108.0));
        assert_eq!(bl.center, dvec2(12.0, 0.0));
    }

    #[test]
    fn a_corner_too_small_to_read_as_a_curve_is_not_worth_a_quad() {
        assert_eq!(cap_patch(wide(), TL, 0.0), None);
        assert_eq!(cap_patch(wide(), TL, 0.49), None);
        assert!(cap_patch(wide(), TL, 0.5).is_some());
    }
}
