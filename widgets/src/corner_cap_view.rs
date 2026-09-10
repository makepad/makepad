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
//! caps were not there. A press that lands in the fake corner therefore
//! still reaches the content under it, which is the one place the fake is
//! arguably better than the real thing.
use crate::{makepad_derive_widget::*, makepad_draw::*, view::View, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // One corner patch: the chrome colour everywhere OUTSIDE a quarter
    // circle and nothing at all inside it, so four of them laid on the
    // corners of a rectangle leave a rounded shape behind. `length` to the
    // arc centre is an exact distance, so clamping it across one point is
    // the whole antialiasing — the same feathering the chart marks use,
    // and premultiplied by hand for the same reason: this builds its
    // colour rather than filling a shape, so nothing else will.
    set_type_default() do #(DrawCornerPatch::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let p = self.pos * self.rect_size
            let d = length(p - vec2(self.center.x, self.center.y)) - self.arc_r
            let aa = clamp(d + 0.5, 0.0, 1.0)
            return Pal.premul(vec4(self.cap_color.xyz, self.cap_color.w * aa))
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

/// A radius under this is thinner than the antialiasing band: the patch
/// would paint nothing anyone can see and still cost a draw call.
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
    let limit = size.x.min(size.y) * 0.5;
    if !(limit > 0.0) {
        return [0.0; 4];
    }
    let shared = if radius > 0.0 { radius } else { 0.0 };
    let mut out = [0.0; 4];
    for (slot, corner) in out.iter_mut().zip(corners) {
        let r = if corner < 0.0 { shared } else { corner };
        // Written as a positive test so a NaN radius falls out as no cap
        // rather than through `clamp`, which would carry it into the quad.
        *slot = if r > 0.0 { r.min(limit) } else { 0.0 };
    }
    out
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
    #[rust]
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
            let rect = self.view.area().rect(cx);
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
        assert!(r[TL] + r[TR] <= 200.0, "the two top caps still fit across the top");
    }

    #[test]
    fn a_rect_with_no_area_is_capped_with_nothing() {
        assert_eq!(cap_radii(8.0, [-1.0; 4], dvec2(0.0, 100.0)), [0.0; 4]);
        assert_eq!(cap_radii(8.0, [-1.0; 4], dvec2(200.0, -3.0)), [0.0; 4]);
    }

    #[test]
    fn a_shared_radius_that_is_negative_or_not_a_number_reads_as_no_rounding() {
        assert_eq!(cap_radii(-4.0, [-1.0; 4], wide().size), [0.0; 4]);
        assert_eq!(cap_radii(f64::NAN, [-1.0; 4], wide().size), [0.0; 4]);
        assert_eq!(cap_radii(8.0, [f64::NAN, 4.0, 4.0, 4.0], wide().size), [0.0, 4.0, 4.0, 4.0]);
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
    fn a_corner_thinner_than_the_antialiasing_band_is_not_worth_a_quad() {
        assert_eq!(cap_patch(wide(), TL, 0.0), None);
        assert_eq!(cap_patch(wide(), TL, 0.49), None);
        assert!(cap_patch(wide(), TL, 0.5).is_some());
    }
}
