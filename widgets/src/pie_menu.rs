//! PieMenu — a ring of choices around the point it was opened at.
//!
//! A list menu asks the hand for two things at once: a direction and a
//! distance, since every row is a different way away and each one is a thin
//! target to stop on. A ring asks for a DIRECTION and nothing else. Past
//! the dead zone in the middle every wedge is unbounded, so a flick that
//! leaves the hub and stops anywhere out along a wedge picks it, and the
//! same flick means the same thing wherever the ring was opened. That is
//! the whole idea, and it is why the pick reads the angle and never the
//! distance.
//!
//! Both the drawing and the picking measure angles CLOCKWISE FROM STRAIGHT
//! UP, and both get that measure from [`PieRing`]. One measure in one place
//! is what stops the wedge that is seen and the wedge that is got from
//! drifting apart by a half step nobody notices until a flick picks the
//! neighbour.
//!
//! The wedges are two comparisons per pixel in the shader — a radius test
//! and an angle test — rather than outlines walked as paths, which do not
//! paint reliably here.
//!
//! What it deliberately does NOT do. It does not remember what was picked:
//! it reports a choice and closes, and a control that has to show its
//! current setting is a radio group and not a menu. It has no submenus — a
//! ring that opens another ring throws away the direction the first one
//! just taught the hand. And it has no arrow keys: the first nine wedges
//! are picked by their own number, because in a ring an arrow would have to
//! mean a compass direction and a step through a list at the same time.

use crate::{badge::measure, makepad_derive_widget::*, makepad_draw::*, widget::*};
use std::f64::consts::TAU;

/// A ring of `count` wedges around a centre, with nothing pickable within
/// `dead_zone` points of that centre.
///
/// Angles here are radians measured CLOCKWISE FROM STRAIGHT UP, which is
/// neither convention this file would otherwise be pulled between: the
/// pixel grid grows downward and `atan2` counts anticlockwise from the +x
/// axis. Converting once, here, is what keeps the drawing and the picking
/// talking about the same wedge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PieRing {
    pub count: usize,
    pub dead_zone: f64,
}

impl PieRing {
    pub fn new(count: usize, dead_zone: f64) -> Self {
        Self { count, dead_zone: dead_zone.max(0.0) }
    }

    /// The angle one wedge occupies. An empty ring reports the whole circle
    /// rather than dividing by nothing; callers check `count` first anyway.
    pub fn step(&self) -> f64 {
        if self.count == 0 {
            TAU
        } else {
            TAU / self.count as f64
        }
    }

    /// The middle of wedge `i`. Wedge 0 points at twelve o'clock and the
    /// rest follow it clockwise, so the numbers read the way a clock face
    /// does and the seam between the last wedge and the first lands ON
    /// twelve rather than beside it.
    pub fn centre(&self, i: usize) -> f64 {
        (i as f64 * self.step()).rem_euclid(TAU)
    }

    /// Where wedge `i` begins and ends, both in `0..TAU`, going clockwise
    /// from the first to the second.
    ///
    /// For the wedge that straddles twelve o'clock the first number is the
    /// LARGER one. That seam is real and a caller that assumes the first is
    /// smaller draws the rest of the circle instead of one wedge.
    pub fn span(&self, i: usize) -> (f64, f64) {
        let half = self.step() * 0.5;
        let c = self.centre(i);
        ((c - half).rem_euclid(TAU), (c + half).rem_euclid(TAU))
    }

    /// The wedge a pointer `dx` to the right of and `dy` BELOW the centre
    /// picks, or nothing when the ring is empty or the pointer has not left
    /// the dead zone.
    ///
    /// How far out the pointer is past the dead zone is not read at all.
    /// That is the point of a ring: the wedge goes on forever, so the hand
    /// has one thing to get right instead of two, and a fast flick that
    /// overshoots by a hundred points picks what a careful one does.
    pub fn pick(&self, dx: f64, dy: f64) -> Option<usize> {
        if self.count == 0 {
            return None;
        }
        if dx * dx + dy * dy < self.dead_zone * self.dead_zone {
            return None;
        }
        let step = self.step();
        // Clockwise from straight up: the screen's y grows downward, so up
        // is -y.
        let a = dx.atan2(-dy).rem_euclid(TAU);
        // Half a step of grace ahead of each centre, which is what puts
        // wedge 0 EITHER side of twelve o'clock instead of starting there.
        // A direction landing exactly on a seam falls to one of the two
        // wedges beside it — which one is not worth promising, since a seam
        // is thinner than the smallest movement a hand can make — but it
        // never falls to nothing.
        Some((((a + step * 0.5) / step).floor() as usize) % self.count)
    }

    /// How far to the right of and BELOW the centre the middle of wedge `i`
    /// sits, `radius` out.
    pub fn offset(&self, i: usize, radius: f64) -> (f64, f64) {
        let a = self.centre(i);
        (a.sin() * radius, -a.cos() * radius)
    }
}

/// What a ring reports: a wedge by its position, counting from zero, the
/// same way the labels are counted.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum PieAction {
    Picked(usize),
    /// Escape, a press outside, or a release back in the dead zone.
    Cancelled,
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DrawPieWedgeBase = #(DrawPieWedge::script_component(vm))
    set_type_default() do #(DrawPieWedge::script_shader(vm)){
        ..mod.draw.DrawQuad

        // Rust writes one of these per wedge, so each is a plain literal.
        a0: 0.0
        a1: 0.0
        hot: 0.0
        inner: 30.0
        outer: 96.0
        grow: 1.0

        /** the face of a wedge the pointer is not aimed at */
        color_wedge: uniform(theme.color_surface_container_high)
        /** the face of the wedge the pointer is aimed at */
        color_wedge_hot: uniform(theme.color_primary)

        pixel: fn() {
            let c = self.rect_size * 0.5
            let p = self.pos * self.rect_size - c
            let r = length(p)
            // The ring grows outward from the hub, so the hole is the right
            // size from the first frame and only the reach animates.
            let outer = self.inner + (self.outer - self.inner) * self.grow
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
            // one distance in pixels feathers all four edges the same.
            let edge = min(min(r - self.inner, outer - r), min(d0, d1) * r)
            let aa = clamp(edge, 0.0, 1.5) / 1.5
            let col = self.color_wedge.mix(self.color_wedge_hot, self.hot)
            // Premultiplied, like everything an Sdf2d returns: this shader
            // builds its colour by hand rather than filling a shape, so it
            // has to do for itself what fill() would have done, or the
            // feathered edges come back brighter than the wedge they edge.
            return Pal.premul(vec4(col.xyz, col.w * aa * self.grow))
        }
    }

    mod.widgets.DrawPieHubBase = #(DrawPieHub::script_component(vm))
    set_type_default() do #(DrawPieHub::script_shader(vm)){
        ..mod.draw.DrawQuad

        radius: 30.0
        grow: 1.0

        /** the face of the dead zone */
        color_hub: uniform(theme.color_surface_container)
        /** the line around it, which is where the choices begin */
        color_hub_edge: uniform(theme.color_outline)

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let c = self.rect_size * 0.5
            sdf.circle(c.x, c.y, self.radius)
            sdf.fill_keep(self.color_hub)
            sdf.stroke(self.color_hub_edge, 1.0)
            // Premultiplied already, so the fade scales all four together.
            return sdf.result * self.grow
        }
    }

    mod.widgets.DrawPieFieldBase = #(DrawPieField::script_component(vm))
    set_type_default() do #(DrawPieField::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            // Nothing: the field is the room a ring may open in, not a
            // surface. Everything drawn is placed absolutely and leaves no
            // rect behind, so without this the widget has nothing to be
            // pressed on, to redraw, or to hold the keyboard with.
            return vec4(0.0, 0.0, 0.0, 0.0)
        }
    }

    mod.widgets.PieMenuBase = #(PieMenu::register_widget(vm))

    /** A ring of choices around the point it was opened at. The pointer's
     * direction from that point picks one, and once past the dead zone in
     * the middle the distance does not matter, so a flick in a direction is
     * the whole gesture. */
    mod.widgets.PieMenu = set_type_default() do mod.widgets.PieMenuBase{
        // Fit, and the widget turns that into the ring's own box: a Fit
        // field has no children to measure, and a ring laid out into
        // nothing is placed and never painted.
        width: Fit
        height: Fit
        /** the choices, the first at twelve o'clock and the rest clockwise */
        labels: []
        /** how far the ring reaches from its centre 40..200 step 2 */
        radius: 96.
        /** the hole in the middle, which is also the dead zone 10..80 step 2 */
        hub_radius: 30.
        /** the gap drawn between two wedges, in degrees 0..12 step 0.5 */
        gap: 2.
        /** a press in the field opens the ring where the press landed 0..1 step 1 */
        open_on_press: true
        /** keep the ring up, centred in the field, and leave it up after a pick 0..1 step 1 */
        pinned: false
        /** number the first nine wedges with the key that picks them 0..1 step 1 */
        show_numbers: true
        /** how long the ring takes to grow open, in seconds 0..0.4 step 0.01 */
        grow_time: 0.12

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
        draw_number +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.type_label_s_size line_spacing: 1.0}
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPieWedge {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    a0: f32,
    #[live]
    a1: f32,
    #[live]
    hot: f32,
    #[live]
    inner: f32,
    #[live]
    outer: f32,
    #[live]
    grow: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPieHub {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    radius: f32,
    #[live]
    grow: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPieField {
    #[deref]
    draw_super: DrawQuad,
}

/// The room left around the ring inside its own field: the rim is
/// feathered over about a pixel and a half, and a ring drawn hard against
/// the field's edge loses that feather to the clip.
const RING_PAD: f64 = 4.0;

/// The least a ring can reach past its hub and still be aimed at.
const RING_MIN: f64 = 8.0;

/// How far below the top of its line box a glyph's ink starts, as a share
/// of the font size. `draw_abs` takes the line box, so centring the box
/// alone leaves every label riding high.
const INK_DROP: f64 = 0.30;

/// Where a wedge's number sits between the hub and the rim, as a share of
/// the distance between them: near the hub, far enough under the word not to
/// read as part of it.
const NUMBER_AT: f64 = 0.22;

/// A ring that is up: where its centre is, when it got there, and which
/// wedge the pointer is aimed at.
#[derive(Clone, Copy, Debug)]
struct Open {
    centre: DVec2,
    opened_at: f64,
    hot: Option<usize>,
    /// True from the press that opened the ring until that press is
    /// released. Without it the release of the opening press — which lands
    /// in the dead zone, because that is where the ring appeared around it
    /// — reads as "released on nothing: cancel", and a click could never
    /// open a ring at all.
    opening_press: bool,
}

#[derive(Script, ScriptHook, Widget)]
pub struct PieMenu {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    /// The field's own rect, drawn as nothing — see the shader's comment.
    #[redraw]
    #[live]
    draw_bg: DrawPieField,
    #[live]
    pub draw_wedge: DrawPieWedge,
    #[live]
    pub draw_hub: DrawPieHub,
    #[live]
    pub draw_label: DrawText,
    #[live]
    pub draw_label_hot: DrawText,
    #[live]
    pub draw_number: DrawText,

    #[live]
    pub labels: Vec<String>,
    #[live(96.0)]
    pub radius: f64,
    #[live(30.0)]
    pub hub_radius: f64,
    #[live(2.0)]
    pub gap: f64,
    #[live(true)]
    pub open_on_press: bool,
    #[live(false)]
    pub pinned: bool,
    #[live(true)]
    pub show_numbers: bool,
    #[live(0.12)]
    pub grow_time: f64,

    #[rust]
    open: Option<Open>,
    #[rust]
    area: Area,
    #[rust]
    next_frame: NextFrame,
}

/// The wedge a key names, counting from one, or nothing. The digits pick by
/// POSITION, which is the only keyboard a ring can honestly have.
fn key_number(code: KeyCode) -> Option<usize> {
    match code {
        KeyCode::Key1 | KeyCode::Numpad1 => Some(1),
        KeyCode::Key2 | KeyCode::Numpad2 => Some(2),
        KeyCode::Key3 | KeyCode::Numpad3 => Some(3),
        KeyCode::Key4 | KeyCode::Numpad4 => Some(4),
        KeyCode::Key5 | KeyCode::Numpad5 => Some(5),
        KeyCode::Key6 | KeyCode::Numpad6 => Some(6),
        KeyCode::Key7 | KeyCode::Numpad7 => Some(7),
        KeyCode::Key8 | KeyCode::Numpad8 => Some(8),
        KeyCode::Key9 | KeyCode::Numpad9 => Some(9),
        _ => None,
    }
}

/// The centre a ring of `radius` opens at when it was asked for at `at`:
/// nudged until the whole ring is inside `field`, because a wedge that is
/// half outside the field is a choice that cannot be aimed at.
///
/// A field too small to hold the ring gets it centred and overflowing.
/// Refusing to draw would leave a press with no menu at all, which is
/// worse than a ring that reaches past its room.
pub fn fit_centre(field: Rect, at: DVec2, radius: f64) -> DVec2 {
    let reach = radius + RING_PAD;
    let axis = |lo: f64, size: f64, want: f64| -> f64 {
        let near = lo + reach;
        let far = lo + size - reach;
        if near > far {
            lo + size * 0.5
        } else {
            want.clamp(near, far)
        }
    };
    dvec2(
        axis(field.pos.x, field.size.x, at.x),
        axis(field.pos.y, field.size.y, at.y),
    )
}

impl PieMenu {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// The wedge the pointer is aimed at right now, or nothing.
    pub fn hot(&self) -> Option<usize> {
        self.open.and_then(|open| open.hot)
    }

    fn ring(&self) -> PieRing {
        PieRing::new(self.labels.len(), self.hub_radius)
    }

    /// The ring's reach, never less than a usable step past the hub: a
    /// caller that sets a hub wider than the radius still gets something
    /// aimable rather than an empty box.
    fn reach(&self) -> f64 {
        self.radius.max(self.hub_radius + RING_MIN)
    }

    /// Open a ring around `at`, in absolute coordinates.
    pub fn open_at(&mut self, cx: &mut Cx, at: DVec2) {
        if self.labels.is_empty() {
            return;
        }
        let field = self.area.rect(cx);
        self.open = Some(Open {
            centre: fit_centre(field, at, self.reach()),
            opened_at: cx.seconds_since_app_start(),
            hot: None,
            opening_press: true,
        });
        self.next_frame = cx.new_next_frame();
        self.redraw(cx);
    }

    /// Take the ring down without reporting anything. A pinned ring stays:
    /// it is a control on the surface and there is nothing to dismiss.
    pub fn close(&mut self, cx: &mut Cx) {
        if self.open.is_some() && !self.pinned {
            self.open = None;
            self.redraw(cx);
        }
    }

    pub fn set_labels(&mut self, cx: &mut Cx, labels: Vec<String>) {
        if self.labels == labels {
            return;
        }
        self.labels = labels;
        if let Some(open) = self.open.as_mut() {
            // The ring has a different number of wedges now, so whatever
            // was hot is a wedge that no longer exists.
            open.hot = None;
        }
        self.redraw(cx);
    }

    /// The wedge an absolute point aims at, from the centre the ring was
    /// last drawn around.
    fn wedge_at(&self, at: DVec2) -> Option<usize> {
        let open = self.open.as_ref()?;
        self.ring().pick(at.x - open.centre.x, at.y - open.centre.y)
    }

    fn set_hot(&mut self, cx: &mut Cx, hot: Option<usize>) {
        let changed = match self.open.as_mut() {
            Some(open) if open.hot != hot => {
                open.hot = hot;
                true
            }
            _ => false,
        };
        if changed {
            self.redraw(cx);
        }
    }

    fn choose(&mut self, cx: &mut Cx, index: usize) {
        if index >= self.labels.len() {
            return;
        }
        let uid = self.uid;
        if !self.pinned {
            self.open = None;
        }
        self.redraw(cx);
        cx.widget_action(uid, PieAction::Picked(index));
    }

    fn cancel(&mut self, cx: &mut Cx) {
        if self.open.is_none() || self.pinned {
            return;
        }
        self.open = None;
        self.redraw(cx);
        let uid = self.uid;
        cx.widget_action(uid, PieAction::Cancelled);
    }

    fn draw_ring(&mut self, cx: &mut Cx2d, open: Open, labels: &[String]) {
        let ring = PieRing::new(labels.len(), self.hub_radius);
        let inner = self.hub_radius.max(1.0);
        let outer = self.reach();
        let grow = if self.grow_time <= 0.0 {
            1.0
        } else {
            ((cx.seconds_since_app_start() - open.opened_at) / self.grow_time).clamp(0.0, 1.0)
        };
        let box_size = (outer + RING_PAD) * 2.0;
        let rect = Rect {
            pos: dvec2(open.centre.x - box_size * 0.5, open.centre.y - box_size * 0.5),
            size: dvec2(box_size, box_size),
        };

        // The drawn gap is cosmetic and the PICK has none: a flick that
        // lands exactly in a seam still has a nearest wedge, and a ring
        // that answers "nothing" to a decisive flick is worse than one that
        // rounds. Capped well short of the step so a crowded ring keeps
        // some wedge left to draw.
        let half_gap = (self.gap.to_radians() * 0.5).min(ring.step() * 0.4);
        for i in 0..labels.len() {
            let (a0, a1) = ring.span(i);
            self.draw_wedge.a0 = (a0 + half_gap).rem_euclid(TAU) as f32;
            self.draw_wedge.a1 = (a1 - half_gap).rem_euclid(TAU) as f32;
            self.draw_wedge.hot = if open.hot == Some(i) { 1.0 } else { 0.0 };
            self.draw_wedge.inner = inner as f32;
            self.draw_wedge.outer = outer as f32;
            self.draw_wedge.grow = grow as f32;
            self.draw_wedge.draw_abs(cx, rect);
        }

        self.draw_hub.radius = inner as f32;
        self.draw_hub.grow = grow as f32;
        self.draw_hub.draw_abs(cx, rect);

        // Words only once the ring has finished growing: a label drawn over
        // a wedge that has not reached it yet hangs in empty space.
        if grow < 1.0 {
            return;
        }
        let mid = (inner + outer) * 0.5;
        for (i, label) in labels.iter().enumerate() {
            let (ox, oy) = ring.offset(i, mid);
            let hot = open.hot == Some(i);
            let draw = if hot { &mut self.draw_label_hot } else { &mut self.draw_label };
            let size = draw.text_style.font_size as f64;
            let width = measure(draw, cx, label);
            let pos = dvec2(
                open.centre.x + ox - width * 0.5,
                open.centre.y + oy - size * 0.5 - size * INK_DROP,
            );
            draw.draw_abs(cx, pos, label);
        }

        if self.show_numbers {
            let at = inner + (outer - inner) * NUMBER_AT;
            // Nine and no further: there is no tenth digit key, and a
            // number under a wedge that no key reaches is a lie.
            for i in 0..labels.len().min(9) {
                let (ox, oy) = ring.offset(i, at);
                let text = (i + 1).to_string();
                let size = self.draw_number.text_style.font_size as f64;
                let width = measure(&self.draw_number, cx, &text);
                let pos = dvec2(
                    open.centre.x + ox - width * 0.5,
                    open.centre.y + oy - size * 0.5 - size * INK_DROP,
                );
                self.draw_number.draw_abs(cx, pos, &text);
            }
        }
    }
}

impl Widget for PieMenu {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // A Fit field becomes the ring's own box. Fit has nothing to
        // measure here — every wedge is placed absolutely and leaves no
        // rect — so a Fit field left alone is nothing wide, and a ring in
        // it is laid out and never painted.
        let natural = (self.reach() + RING_PAD) * 2.0;
        let walk = Walk {
            width: match walk.width {
                Size::Fit { .. } => Size::Fixed(natural),
                other => other,
            },
            height: match walk.height {
                Size::Fit { .. } => Size::Fixed(natural),
                other => other,
            },
            ..walk
        };
        self.draw_bg.begin(cx, walk, self.layout);
        let field = cx.turtle().rect();

        if self.pinned && self.open.is_none() && !self.labels.is_empty() {
            self.open = Some(Open {
                centre: field.center(),
                opened_at: cx.seconds_since_app_start(),
                hot: None,
                opening_press: false,
            });
            self.next_frame = cx.new_next_frame();
        }

        if let Some(mut open) = self.open {
            // Re-fitted every draw, not just at the open: the field may
            // have been resized under the ring, and a ring half outside its
            // field has choices that cannot be aimed at.
            open.centre = fit_centre(field, open.centre, self.reach());
            self.open = Some(open);
            let labels = self.labels.clone();
            if !labels.is_empty() {
                self.draw_ring(cx, open, &labels);
                if self.grow_time > 0.0
                    && cx.seconds_since_app_start() - open.opened_at < self.grow_time
                {
                    self.next_frame = cx.new_next_frame();
                }
            }
        }

        self.draw_bg.end(cx);
        self.area = self.draw_bg.area();
        // One stop for the ring, so Tab reaches it and the number keys work
        // without the pointer being anywhere near.
        cx.add_nav_stop(self.area, NavRole::TextInput, Inset::default());
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            self.redraw(cx);
        }
        // A press that lands outside an open ring dismisses it. `hits` only
        // reports what lands on the field, so the raw press is the only
        // place a press that missed can be seen.
        if let Event::MouseDown(me) = event {
            if self.open.is_some() && !self.area.rect(cx).contains(me.abs) {
                self.cancel(cx);
            }
        }
        match event.hits(cx, self.area) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                cx.set_key_focus(self.area);
                if self.open.is_none() {
                    if self.open_on_press {
                        self.open_at(cx, fe.abs);
                    }
                } else {
                    let hot = self.wedge_at(fe.abs);
                    self.set_hot(cx, hot);
                }
            }
            // A drag is the flick: the pointer keeps reporting after it has
            // left the field, which is exactly what "the distance does not
            // matter" needs.
            Hit::FingerMove(fe) => {
                let hot = self.wedge_at(fe.abs);
                self.set_hot(cx, hot);
            }
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hot = self.wedge_at(fe.abs);
                if self.open.is_some() {
                    cx.set_cursor(if hot.is_some() {
                        MouseCursor::Hand
                    } else {
                        MouseCursor::Default
                    });
                }
                self.set_hot(cx, hot);
            }
            Hit::FingerHoverOut(_) => self.set_hot(cx, None),
            Hit::FingerUp(fe) if fe.is_primary_hit() => {
                if let Some(open) = self.open {
                    match self.wedge_at(fe.abs) {
                        Some(index) => self.choose(cx, index),
                        None if open.opening_press => {
                            // The press that opened the ring, let go in the
                            // dead zone it opened around: the ring stays up
                            // and the reader picks with a second gesture.
                            // The same press dragged out and released picks
                            // in one.
                            if let Some(open) = self.open.as_mut() {
                                open.opening_press = false;
                            }
                        }
                        None => self.cancel(cx),
                    }
                }
            }
            Hit::KeyDown(ke) => match ke.key_code {
                KeyCode::Escape => self.cancel(cx),
                code => {
                    if let Some(n) = key_number(code) {
                        if self.open.is_some() && n <= self.labels.len() {
                            self.choose(cx, n - 1);
                        }
                    }
                }
            },
            Hit::KeyFocusLost(_) => self.cancel(cx),
            _ => {}
        }
    }

    /// The wedge being aimed at, so a test can read the ring in one line.
    fn text(&self) -> String {
        self.hot()
            .and_then(|i| self.labels.get(i).cloned())
            .unwrap_or_default()
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(match self.open {
            None => "closed".to_string(),
            Some(open) => match open.hot {
                Some(i) => format!("open {}", i + 1),
                None => "open".to_string(),
            },
        })
    }
}

impl PieMenuRef {
    pub fn is_open(&self) -> bool {
        self.borrow().map(|inner| inner.is_open()).unwrap_or(false)
    }

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

    pub fn set_labels(&self, cx: &mut Cx, labels: Vec<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_labels(cx, labels);
        }
    }

    /// The label at a position, for a caller that wants the word back
    /// rather than the number it reported.
    pub fn label_at(&self, index: usize) -> String {
        self.borrow()
            .and_then(|inner| inner.labels.get(index).cloned())
            .unwrap_or_default()
    }

    /// The wedge picked this pass, if one was.
    pub fn picked(&self, actions: &Actions) -> Option<usize> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<PieAction>() {
            PieAction::Picked(index) => Some(index),
            _ => None,
        }
    }

    /// Whether the ring was taken down this pass without a choice.
    pub fn cancelled(&self, actions: &Actions) -> bool {
        let Some(action) = actions.find_widget_action(self.widget_uid()) else {
            return false;
        };
        matches!(action.cast::<PieAction>(), PieAction::Cancelled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pointer `points` away from the centre, `deg` clockwise from
    /// straight up, in screen coordinates where y grows downward.
    fn aim(deg: f64, points: f64) -> (f64, f64) {
        let a = deg.to_radians();
        (a.sin() * points, -a.cos() * points)
    }

    /// What a ring picks for a direction, well clear of any dead zone.
    fn at(ring: &PieRing, deg: f64) -> Option<usize> {
        let (dx, dy) = aim(deg, 100.0);
        ring.pick(dx, dy)
    }

    /// Inside the hub there is no direction worth reading — the hand has
    /// not said anything yet. Outside it, only the direction is read: the
    /// same aim one point out and a thousand points out is the same wedge,
    /// which is the whole reason a ring beats a list.
    #[test]
    fn the_dead_zone_answers_nothing_and_distance_past_it_says_nothing_more() {
        let ring = PieRing::new(4, 30.0);
        assert_eq!(ring.pick(0.0, 0.0), None, "the centre itself");
        let (dx, dy) = aim(45.0, 29.0);
        assert_eq!(ring.pick(dx, dy), None, "still inside the hub");
        let (dx, dy) = aim(45.0, 31.0);
        assert_eq!(ring.pick(dx, dy), Some(1), "one point out and it answers");
        let (dx, dy) = aim(45.0, 4000.0);
        assert_eq!(ring.pick(dx, dy), Some(1), "a mile out is the same wedge");
    }

    /// Wedge 0 is CENTRED on twelve o'clock, so the seam between the last
    /// wedge and the first is on twelve too. A hair either side of straight
    /// up must be the same wedge; the bug this catches is the wrap being
    /// dropped, which sends the left hair to the last wedge and puts a
    /// fault line down the middle of the most-aimed-at target on the ring.
    #[test]
    fn the_seam_between_the_last_wedge_and_the_first_is_at_twelve_oclock() {
        let ring = PieRing::new(4, 10.0);
        assert_eq!(at(&ring, 0.0), Some(0), "straight up");
        assert_eq!(at(&ring, 0.5), Some(0), "a hair clockwise of up");
        assert_eq!(at(&ring, -0.5), Some(0), "a hair anticlockwise of up");
        assert_eq!(at(&ring, 44.9), Some(0), "just short of the first seam");
        assert_eq!(at(&ring, 45.1), Some(1), "just over it");
        assert_eq!(at(&ring, 314.9), Some(3), "just short of the last seam");
        assert_eq!(at(&ring, 315.1), Some(0), "and over that one, back to the first");
    }

    /// An odd ring has no wedge opposite another and no seam on any axis,
    /// which is where an even-count assumption shows up. Every wedge's own
    /// direction picks itself, and both of its seams belong to the right
    /// side.
    #[test]
    fn an_odd_ring_still_lands_every_wedge_on_its_own_direction() {
        let ring = PieRing::new(5, 20.0);
        for i in 0..5 {
            let centre = i as f64 * 72.0;
            assert_eq!(at(&ring, centre), Some(i), "the middle of wedge {i}");
            assert_eq!(at(&ring, centre + 35.9), Some(i), "its clockwise edge");
            assert_eq!(at(&ring, centre - 35.9), Some(i), "its other edge");
            assert_eq!(at(&ring, centre + 36.1), Some((i + 1) % 5), "over the seam");
        }
        // Three, for the one direction most likely to be assumed: right.
        let three = PieRing::new(3, 20.0);
        assert_eq!(at(&three, 0.0), Some(0), "up");
        assert_eq!(at(&three, 90.0), Some(1), "right");
        assert_eq!(at(&three, 270.0), Some(2), "left");
    }

    /// The wedge that is drawn and the wedge that is picked are the same
    /// wedge: every span's own middle picks the span it came from, and each
    /// span ends exactly where the next begins, so the ring has no gap for
    /// a flick to fall into.
    #[test]
    fn the_drawn_spans_tile_the_circle_and_agree_with_the_picking() {
        for count in [1usize, 2, 3, 5, 8] {
            let ring = PieRing::new(count, 20.0);
            for i in 0..count {
                let (a0, a1) = ring.span(i);
                let mid = (a0 + (a1 - a0).rem_euclid(TAU) * 0.5).rem_euclid(TAU);
                let (dx, dy) = aim(mid.to_degrees(), 100.0);
                assert_eq!(ring.pick(dx, dy), Some(i), "the middle of span {i} of {count}");
                let next = ring.span((i + 1) % count).0;
                assert!(
                    (a1 - next).abs() < 1e-9,
                    "span {i} of {count} ends at {a1}, the next begins at {next}"
                );
            }
        }
    }

    /// One choice is a whole circle, and an empty ring answers nothing at
    /// all rather than a wedge that is not there.
    #[test]
    fn a_ring_of_one_takes_every_direction_and_a_ring_of_none_takes_none() {
        let one = PieRing::new(1, 15.0);
        for deg in [0.0, 90.0, 180.0, 270.0, 359.0] {
            assert_eq!(at(&one, deg), Some(0), "{deg} degrees");
        }
        assert_eq!(one.pick(0.0, 0.0), None, "even so, not from the hub");
        let none = PieRing::new(0, 15.0);
        assert_eq!(at(&none, 90.0), None, "nothing to pick");
    }

    /// Wedge 0's label goes straight above the centre, and the offsets run
    /// clockwise from it. A sign error here draws the ring mirrored, which
    /// reads as correct until the picking disagrees with it.
    #[test]
    fn the_first_wedge_sits_above_the_centre_and_the_rest_run_clockwise() {
        let ring = PieRing::new(4, 20.0);
        let (dx, dy) = ring.offset(0, 50.0);
        assert!(dx.abs() < 1e-9 && (dy + 50.0).abs() < 1e-9, "up is ({dx}, {dy})");
        let (dx, dy) = ring.offset(1, 50.0);
        assert!((dx - 50.0).abs() < 1e-9 && dy.abs() < 1e-9, "right is ({dx}, {dy})");
        let (dx, dy) = ring.offset(2, 50.0);
        assert!(dx.abs() < 1e-9 && (dy - 50.0).abs() < 1e-9, "down is ({dx}, {dy})");
    }

    /// A ring opened near an edge is nudged until all of it is in the
    /// field, because a wedge that is half outside cannot be aimed at. A
    /// field too small for the ring centres it rather than refusing.
    #[test]
    fn a_ring_opened_at_the_edge_is_nudged_until_all_of_it_is_reachable() {
        let field = Rect { pos: dvec2(0.0, 0.0), size: dvec2(400.0, 300.0) };
        let middle = fit_centre(field, dvec2(200.0, 150.0), 50.0);
        assert_eq!(middle, dvec2(200.0, 150.0), "room to spare: left where it was asked for");
        let corner = fit_centre(field, dvec2(2.0, 2.0), 50.0);
        assert_eq!(corner, dvec2(54.0, 54.0), "pushed in by the reach and the pad");
        let far = fit_centre(field, dvec2(399.0, 299.0), 50.0);
        assert_eq!(far, dvec2(346.0, 246.0), "and in from the other two edges");
        let cramped = Rect { pos: dvec2(10.0, 10.0), size: dvec2(40.0, 40.0) };
        assert_eq!(
            fit_centre(cramped, dvec2(12.0, 12.0), 50.0),
            dvec2(30.0, 30.0),
            "no room at all: centred, and let to overflow"
        );
    }
}
