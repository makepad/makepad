//! The pointer's field over a control, for a material that answers the
//! pointer's approach (a magnetic liquid whose spikes rise as it comes
//! near): a spring toward one while the pointer is within reach, the
//! pointer's place in the control's frame, and a clock that runs while the
//! field is up. And the arbiter that keeps the cost down: at any pointer
//! position only the nearest two controls carry a field, so one can fade
//! out while the next fades in and the rest stay still.
//!
//! For a material that answers the pointer's movement rather than its
//! presence (water that rings where the pointer passed), the trail: the
//! last few places the pointer moved through, each with its age, alive for
//! a set time after the pointer last moved. And the glass over a display
//! that carries both, `PointerGlass`, which lifts itself over the content
//! under it while the trail is alive, so its shader can read that content
//! and bend it.
use crate::{
    gauss_view::{arm_gauss_capture, bind_gauss_snapshot, request_window_scene},
    *,
};
use std::cell::RefCell;

/// The two nearest reports at a pointer position decide who carries the
/// field; the reports of the position before decide for the current one,
/// so every control sees the same answer whatever order they report in,
/// one pointer event late, which is never seen.
#[derive(Default)]
pub struct FieldArbiter {
    at: Option<DVec2>,
    cur: [(f64, u64); 2],
    cur_n: usize,
    prev: [(f64, u64); 2],
    prev_n: usize,
}

thread_local! {
    static ARBITER: RefCell<FieldArbiter> = RefCell::new(FieldArbiter::default());
}

impl FieldArbiter {
    /// A control reports its distance to the pointer at `abs` (points,
    /// zero inside it) under its own id, and learns whether it is one of
    /// the two nearest. The UI thread alone calls this; no lock is taken.
    pub fn report(abs: DVec2, id: u64, dist: f64) -> bool {
        ARBITER.with(|a| a.borrow_mut().report_in(abs, id, dist))
    }

    fn report_in(&mut self, abs: DVec2, id: u64, dist: f64) -> bool {
        let moved = match self.at {
            Some(p) => (p.x - abs.x).abs() > 0.01 || (p.y - abs.y).abs() > 0.01,
            None => true,
        };
        if moved {
            self.prev = self.cur;
            self.prev_n = self.cur_n;
            self.cur_n = 0;
            self.at = Some(abs);
        }
        // Keep the two smallest distances for this position.
        let mut slot = None;
        for i in 0..self.cur_n {
            if self.cur[i].1 == id {
                slot = Some(i);
            }
        }
        match slot {
            Some(i) => self.cur[i].0 = dist,
            None => {
                if self.cur_n < 2 {
                    self.cur[self.cur_n] = (dist, id);
                    self.cur_n += 1;
                } else {
                    let far = if self.cur[0].0 >= self.cur[1].0 { 0 } else { 1 };
                    if dist < self.cur[far].0 {
                        self.cur[far] = (dist, id);
                    }
                }
            }
        }
        // The decision: among the nearest two of the position before, or
        // nobody has reported yet.
        if self.prev_n < 2 {
            return true;
        }
        (0..self.prev_n).any(|i| self.prev[i].1 == id)
    }
}

/// The field itself, stepped on the frames after a kick until it rests.
#[derive(Default)]
pub struct PointerField {
    hover: f64,
    hover_v: f64,
    target: f64,
    pointer: Option<(f64, f64)>,
    /// Where the field is drawn: the pointer followed on a critically
    /// damped spring, so it glides after the pointer instead of jumping
    /// with each pointer event; held at the pointer while the control is
    /// pressed, and taken straight to it while the field is down.
    shown: Option<(f64, f64)>,
    shown_v: (f64, f64),
    time: f64,
    next_frame: Option<NextFrame>,
    last_time: Option<f64>,
    /// The frames stop once the field is still, though the pointer is in
    /// reach; the clock stops with them.
    rests_when_still: bool,
}

impl PointerField {
    /// The pointer read against the control: `rel` its place in the
    /// control's frame in points from the centre, `near` whether it is
    /// within reach and the control may answer it. Asks for a frame when
    /// anything changed that the material would show.
    pub fn pointer(&mut self, cx: &mut Cx, rel: (f64, f64), near: bool) {
        let target = if near { 1.0 } else { 0.0 };
        let moved = match self.pointer {
            Some(p) => (p.0 - rel.0).abs() > 0.01 || (p.1 - rel.1).abs() > 0.01,
            None => true,
        };
        self.pointer = Some(rel);
        if target != self.target || (moved && self.strength(0.0) > 0.001) {
            self.target = target;
            self.kick(cx);
        }
    }

    /// The pointer left the window: the field falls.
    pub fn leave(&mut self, cx: &mut Cx) {
        if self.target != 0.0 {
            self.target = 0.0;
            self.kick(cx);
        }
    }

    /// Starts the frames, if they are not running.
    pub fn kick(&mut self, cx: &mut Cx) {
        if self.next_frame.is_none() {
            self.last_time = None;
            self.next_frame = Some(cx.new_next_frame());
        }
    }

    /// One step on the field's frame; true when a step ran, so the caller
    /// hands the material the new values. `press` is the control's own
    /// press 0..1, which joins the field.
    pub fn tick(&mut self, cx: &mut Cx, event: &Event, press: f64) -> bool {
        let Some(nf) = self.next_frame else {
            return false;
        };
        let Some(ne) = nf.is_event(event) else {
            return false;
        };
        self.next_frame = None;
        let dt = match self.last_time {
            Some(t) => (ne.time - t).clamp(0.001, 0.05),
            None => 1.0 / 60.0,
        };
        self.last_time = Some(ne.time);
        // Critically damped, so it settles without crossing zero.
        let a = 40.0 * (self.target - self.hover) - 12.7 * self.hover_v;
        self.hover_v += a * dt;
        self.hover += self.hover_v * dt;
        self.hover = self.hover.max(0.0);
        if self.strength(press) > 0.001 {
            self.time += dt;
        }
        if let Some(p) = self.pointer {
            let snap = press > 0.5 || self.strength(press) < 0.001;
            self.shown_v = crate::slider::field_follow(&mut self.shown, self.shown_v, p, snap, dt);
        }
        let held = if self.rests_when_still {
            self.gliding()
        } else {
            self.strength(press) > 0.001
        };
        let active = self.hover_v.abs() > 1e-3 || (self.hover - self.target).abs() > 1e-3 || held;
        if active {
            self.next_frame = Some(cx.new_next_frame());
        }
        true
    }

    /// For a material that answers the pointer's movement and not its
    /// presence: the field's frames stop as soon as it is still (risen or
    /// fallen, and the place it is drawn at caught up with the pointer),
    /// though the pointer stays in reach, and the clock stops with them. A
    /// pointer resting over the control then costs no frames. Off, the
    /// default, the frames run for as long as the field is up.
    pub fn set_rests_when_still(&mut self, rests: bool) {
        self.rests_when_still = rests;
    }

    /// The place the field is drawn at is still on its way to the pointer.
    fn gliding(&self) -> bool {
        match (self.shown, self.pointer) {
            (Some(s), Some(p)) => {
                (s.0 - p.0).abs() > 0.05
                    || (s.1 - p.1).abs() > 0.05
                    || self.shown_v.0.abs() > 0.5
                    || self.shown_v.1.abs() > 0.5
            }
            _ => false,
        }
    }

    /// The field's strength: most of the hover plus the press, within 0..1.
    pub fn strength(&self, press: f64) -> f64 {
        (0.7 * self.hover + press).clamp(0.0, 1.0)
    }

    /// What the material reads: the strength, the field's place along and
    /// across the control from its centre (the pointer, followed), and the
    /// clock.
    pub fn read(&self, press: f64) -> (f32, f32, f32, f32) {
        let (a, c) = self.shown.or(self.pointer).unwrap_or((0.0, 0.0));
        (self.strength(press) as f32, a as f32, c as f32, self.time as f32)
    }

    /// True while the field is up or moving.
    pub fn active(&self) -> bool {
        self.next_frame.is_some() || self.hover > 0.001
    }
}

/// How many drops a [`PointerTrail`] keeps.
pub const TRAIL_DROPS: usize = 8;

#[derive(Clone, Copy, Default)]
struct TrailDrop {
    at: (f64, f64),
    born: f64,
    set: bool,
}

/// The pointer's recent path over a control, as drops: the last
/// [`TRAIL_DROPS`] places it moved through, each with the time it was
/// dropped there. A drop is added only when the pointer has moved
/// `spacing` points from the last one, and no sooner than eight tenths of
/// a drop's life divided among the drops, so the oldest drop is all but
/// gone when a new one takes its place; a pointer that stands still drops
/// nothing. The trail runs frames while its newest drop is younger than
/// `life`, and none after.
#[derive(Default)]
pub struct PointerTrail {
    drops: [TrailDrop; TRAIL_DROPS],
    /// The slot the next drop goes into, which holds the oldest.
    next: usize,
    /// Where the pointer was at the last drop, or where it came into reach.
    from: Option<(f64, f64)>,
    last_born: Option<f64>,
    next_frame: Option<NextFrame>,
}

impl PointerTrail {
    /// The pointer within reach at `at` (points, in the control's frame)
    /// at `now` (seconds since the app started). True when it dropped.
    pub fn pointer(&mut self, cx: &mut Cx, at: (f64, f64), now: f64, spacing: f64, life: f64) -> bool {
        let Some(from) = self.from else {
            self.from = Some(at);
            return false;
        };
        let (dx, dy) = (at.0 - from.0, at.1 - from.1);
        if (dx * dx + dy * dy).sqrt() < spacing.max(0.5) || life <= 0.0 {
            return false;
        }
        let gap = life * 0.8 / TRAIL_DROPS as f64;
        if self.last_born.is_some_and(|born| now - born < gap) {
            return false;
        }
        self.drops[self.next] = TrailDrop { at, born: now, set: true };
        self.next = (self.next + 1) % TRAIL_DROPS;
        self.from = Some(at);
        self.last_born = Some(now);
        if self.next_frame.is_none() {
            self.next_frame = Some(cx.new_next_frame());
        }
        true
    }

    /// The pointer went out of reach: the path it comes back on starts
    /// afresh. The drops already made live on.
    pub fn lift(&mut self) {
        self.from = None;
    }

    /// One step on the trail's frame; true when a step ran, so the caller
    /// hands the material the new ages.
    pub fn tick(&mut self, cx: &mut Cx, event: &Event, now: f64, life: f64) -> bool {
        let Some(nf) = self.next_frame else {
            return false;
        };
        if nf.is_event(event).is_none() {
            return false;
        }
        self.next_frame = None;
        if self.alive(now, life) {
            self.next_frame = Some(cx.new_next_frame());
        }
        true
    }

    /// The newest drop is younger than `life`.
    pub fn alive(&self, now: f64, life: f64) -> bool {
        self.last_born.is_some_and(|born| now - born < life)
    }

    /// What the material reads, newest first: each drop's place (points, in
    /// the control's frame) and its age in seconds; a slot with no drop
    /// younger than `life` has the age -1.
    pub fn read(&self, now: f64, life: f64) -> [[f32; 3]; TRAIL_DROPS] {
        let mut out = [[0.0, 0.0, -1.0f32]; TRAIL_DROPS];
        for (k, slot) in out.iter_mut().enumerate() {
            let drop = self.drops[(self.next + TRAIL_DROPS - 1 - k) % TRAIL_DROPS];
            let age = (now - drop.born).max(0.0);
            if drop.set && age < life {
                *slot = [drop.at.0 as f32, drop.at.1 as f32, age as f32];
            }
        }
        out
    }
}

/// What a [`PointerGlass`] is set to, from its widget's properties.
#[derive(Clone, Copy, Debug)]
pub struct PointerGlassSetup {
    /// How far past the glass's edge the pointer is in reach, in points.
    /// Zero turns the field and the trail off.
    pub reach: f64,
    /// How far the pointer moves between drops, in points.
    pub spacing: f64,
    /// How long a drop lives, in seconds.
    pub life: f64,
    /// Lift the glass over the content under it while the trail is alive.
    pub backdrop: bool,
}

/// A sheet of glass drawn over a widget's content that answers the
/// pointer: a display's glass that water rings across where the pointer
/// moved. It carries a [`PointerField`] (one that rests when still) and a
/// [`PointerTrail`] and hands both to the glass's shader as instances, on
/// every frame either steps, written into the drawn glass without a
/// redraw:
///
/// - `pointer_field`, a `vec4`: the field's strength 0..1, the pointer's
///   place (followed) in points from the glass's top left, and the field's
///   clock in seconds;
/// - `trail_0` to `trail_7`, each a `vec3`, newest first: a drop's place in
///   points from the glass's top left and its age in seconds, -1 for no
///   drop;
///
/// and the uniform `trail_life`, the seconds a drop lives. A shader that
/// does not declare them pays nothing for them (a draw holds 32 instance
/// floats: the trail takes 24, the field 4). With
/// `backdrop`, while the trail is alive the glass is drawn on an overlay of
/// its own begun under the plain ones, with the window's scene of this
/// frame bound as [`bind_gauss_snapshot`] binds it, so its shader can read
/// the content under it and bend it (`has_gauss` 1, and slot 0 the scene;
/// a shader declares `scene_texture` as its first texture, and
/// `source_size`, `source_y_flip` and `has_gauss`). It arms the window's
/// capture when the trail comes alive and is drawn back in place, with
/// nothing bound, when the trail dies: at rest it costs no frames and no
/// capture, and draws exactly where it always did. Inside something already
/// on an overlay the content is not in the capture, so the glass stays
/// where it is, with nothing bound. The UI thread alone touches this.
pub struct PointerGlass {
    field: PointerField,
    trail: PointerTrail,
    list: Option<DrawList2d>,
    /// Drawn on its overlay in the last draw.
    lifted: bool,
    /// Its last draw could have lifted it (it was not inside an overlay).
    liftable: bool,
    /// The scene's slots are bound in the glass's draw vars.
    bound: bool,
}

impl Default for PointerGlass {
    fn default() -> Self {
        let mut field = PointerField::default();
        field.set_rests_when_still(true);
        Self {
            field,
            trail: PointerTrail::default(),
            list: None,
            lifted: false,
            liftable: false,
            bound: false,
        }
    }
}

fn trail_ids() -> [LiveId; TRAIL_DROPS] {
    [
        live_id!(trail_0),
        live_id!(trail_1),
        live_id!(trail_2),
        live_id!(trail_3),
        live_id!(trail_4),
        live_id!(trail_5),
        live_id!(trail_6),
        live_id!(trail_7),
    ]
}

impl PointerGlass {
    fn values(&self, now: f64, life: f64) -> ([f32; 4], [[f32; 3]; TRAIL_DROPS]) {
        let (s, x, y, t) = self.field.read(0.0);
        ([s, x, y, t], self.trail.read(now, life))
    }

    /// The pointer and the frames. True when the widget has to draw again:
    /// the glass goes onto its overlay or comes back off it. Otherwise the
    /// new values are written into the drawn glass, which repaints only.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, glass: &mut DrawQuad, setup: &PointerGlassSetup) -> bool {
        if setup.reach <= 0.0 {
            return false;
        }
        let now = cx.seconds_since_app_start();
        let mut write = self.field.tick(cx, event, 0.0);
        let mut redraw = false;
        if self.trail.tick(cx, event, now, setup.life) {
            write = true;
            if self.lifted && !self.trail.alive(now, setup.life) {
                redraw = true;
            }
        }
        match event {
            Event::MouseMove(e) => {
                let rect = glass.draw_vars.area().rect(cx);
                if rect.size.x > 0.0 && rect.size.y > 0.0 {
                    let rel = (e.abs.x - rect.pos.x, e.abs.y - rect.pos.y);
                    let dx = ((rel.0 - rect.size.x * 0.5).abs() - rect.size.x * 0.5).max(0.0);
                    let dy = ((rel.1 - rect.size.y * 0.5).abs() - rect.size.y * 0.5).max(0.0);
                    let near = (dx * dx + dy * dy).sqrt() < setup.reach;
                    self.field.pointer(cx, rel, near);
                    if near {
                        let was_alive = self.trail.alive(now, setup.life);
                        if self.trail.pointer(cx, rel, now, setup.spacing, setup.life) {
                            write = true;
                            if setup.backdrop && self.liftable && !self.lifted && !was_alive {
                                // The window captures on the frame the
                                // lifted glass first paints in.
                                arm_gauss_capture(cx);
                                redraw = true;
                            }
                        }
                    } else {
                        self.trail.lift();
                    }
                }
            }
            Event::MouseLeave(_) => {
                self.field.leave(cx);
                self.trail.lift();
            }
            _ => (),
        }
        if write && !redraw {
            let (field, drops) = self.values(now, setup.life);
            glass.draw_vars.set_instance_on_area(cx, live_id!(pointer_field), &field);
            for (id, drop) in trail_ids().iter().zip(drops.iter()) {
                glass.draw_vars.set_instance_on_area(cx, *id, drop);
            }
        }
        redraw
    }

    /// Draws the glass at `rect`, lifted over the content under it while
    /// the trail is alive and `backdrop` asks for it, in place otherwise.
    pub fn draw(&mut self, cx: &mut Cx2d, glass: &mut DrawQuad, rect: Rect, setup: &PointerGlassSetup) {
        let on = setup.reach > 0.0;
        let now = cx.seconds_since_app_start();
        if on {
            let (field, drops) = self.values(now, setup.life);
            glass.draw_vars.set_uniform(cx, live_id!(trail_life), &[setup.life as f32]);
            glass.draw_vars.set_dyn_instance(cx, live_id!(pointer_field), &field);
            for (id, drop) in trail_ids().iter().zip(drops.iter()) {
                glass.draw_vars.set_dyn_instance(cx, *id, drop);
            }
        }
        self.liftable = !cx.is_drawing_overlay();
        let lift = on && setup.backdrop && self.liftable && self.trail.alive(now, setup.life);
        if lift {
            self.list.get_or_insert_with(|| DrawList2d::new(cx)).begin_overlay_under(cx);
            let snapshot = request_window_scene(cx);
            bind_gauss_snapshot(&mut glass.draw_vars, cx, snapshot);
            self.bound = true;
        } else if self.bound {
            bind_gauss_snapshot(&mut glass.draw_vars, cx, None);
            self.bound = false;
        }
        self.lifted = lift;
        glass.draw_abs(cx, rect);
        if lift {
            if let Some(list) = self.list.as_mut() {
                list.end(cx);
            }
        }
    }
}
