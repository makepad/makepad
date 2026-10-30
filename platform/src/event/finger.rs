#![allow(unused)]
#![allow(dead_code)]
use {
    crate::{
        area::Area,
        cx::Cx,
        event::event::{Event, Hit},
        event::xr::XrHand,
        makepad_live_id::{live_id, live_id_num, FromLiveId},
        makepad_math::*,
        makepad_micro_serde::*,
        makepad_script::*,
        window::WindowId,
    },
    std::{
        cell::{Cell, RefCell},
        ops::Deref,
        rc::{Rc, Weak},
    },
};

// Mouse events

pub use makepad_studio_protocol::{KeyModifiers, MouseButton, PinchPhase};

#[derive(Clone, Debug)]
pub struct MouseDownEvent {
    pub abs: Vec2d,
    pub button: MouseButton,
    pub window_id: WindowId,
    pub modifiers: KeyModifiers,
    pub handled: Cell<Area>,
    pub time: f64,
}

#[derive(Clone, Debug)]
pub struct MouseMoveEvent {
    pub abs: Vec2d,
    /// Relative motion while the pointer is LOCKED (cx.lock_mouse_pointer):
    /// the browser pointer-lock model — `abs` stays pinned at the lock point
    /// (so widget routing is stable) and the true movement arrives here.
    /// Always zero when unlocked or on platforms without lock support.
    pub lock_delta: Vec2d,
    pub window_id: WindowId,
    pub modifiers: KeyModifiers,
    pub time: f64,
    pub handled: Cell<Area>,
}

#[derive(Debug)]
pub struct TweakRayEvent {
    pub abs: Vec2d,
    pub window_id: WindowId,
    pub modifiers: KeyModifiers,
    pub time: f64,
    pub dpi_factor: f64,
    pub hit_widget_uids: RefCell<Vec<u64>>,
    pub hit_rect: Cell<Option<Rect>>,
}

#[derive(Clone, Debug)]
pub struct MouseUpEvent {
    pub abs: Vec2d,
    pub button: MouseButton,
    pub window_id: WindowId,
    pub modifiers: KeyModifiers,
    pub time: f64,
}

#[derive(Clone, Debug)]
pub struct MouseLeaveEvent {
    pub abs: Vec2d,
    pub window_id: WindowId,
    pub modifiers: KeyModifiers,
    pub time: f64,
    pub handled: Cell<Area>,
}

/// The gesture phase of a scroll event, when the OS provides one.
///
/// Trackpad scrolling on some platforms (macOS in particular) is a gesture: the OS reports
/// when fingers touch the pad (`Began`), each movement while they are down (`Changed`), when
/// they lift off (`Ended`), and then a stream of decaying momentum deltas
/// (`Momentum`/`MomentumEnded`).
///
/// The scrollable widgets apply the user-driven deltas directly and, on `Ended`, start a fling
/// that follows the OS momentum stream so the deceleration matches the OS (see
/// `widgets::scroll_motion`). Widgets that don't track phases can apply every delta directly,
/// which gives plain OS momentum scrolling.
///
/// On platforms and devices with no phase information (classic mouse wheels, X11, Windows),
/// this is `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollPhase {
    /// No phase information available (classic mouse wheel, or the platform doesn't report phases).
    #[default]
    None,
    /// Fingers touched the trackpad; a scroll gesture may begin (the delta is usually zero).
    Began,
    /// A raw finger contact on the trackpad, sent for every touch with a zero delta
    /// (macOS only). Unlike `Began`, this fires even for single-finger contacts that never
    /// become a scroll gesture, so widgets use it to instantly stop kinetic scrolling the
    /// way a touch natively catches a coast. It must not disturb anything else: any
    /// tap-to-click press or drag from the same contact arrives separately as mouse events.
    Touched,
    /// Fingers moved while on the trackpad: a user-driven scroll delta.
    Changed,
    /// Fingers lifted off the trackpad: the last event of the user-driven gesture (the delta
    /// may be zero). Widgets start their fling here.
    Ended,
    /// A momentum delta arriving from the OS after the fingers lifted. Widgets follow these to
    /// match the OS deceleration rate.
    Momentum,
    /// The OS momentum stream finished (the delta is zero).
    MomentumEnded,
}

#[derive(Clone, Debug)]
pub struct ScrollEvent {
    pub window_id: WindowId,
    pub scroll: Vec2d,
    pub abs: Vec2d,
    pub modifiers: KeyModifiers,
    pub handled_x: Cell<bool>,
    pub handled_y: Cell<bool>,
    pub is_mouse: bool,
    pub time: f64,
    pub phase: ScrollPhase,
}

impl ScrollEvent {
    pub fn handled(&self, axis: Vec2Index) -> &Cell<bool> {
        match axis {
            Vec2Index::X => &self.handled_x,
            Vec2Index::Y => &self.handled_y,
        }
    }
}

/// Like Chromium's 500 ms wheel transaction, phase-less wheel events form one scroll until they pause.
pub const WHEEL_LATCH_TIMEOUT: f64 = 0.5;
/// Like Firefox, a wheel scroll lets go of an owner the pointer moves off once the wheel rested this long.
pub const WHEEL_LATCH_MOVE_DELAY: f64 = 0.1;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum ScrollGesture {
    #[default]
    Idle,
    Wheel,
    /// Trackpad fingers are scrolling.
    Fingers,
    /// The fingers lifted; the OS momentum stream may follow.
    Lifted,
}

/// Which way a scroll gesture may go, decided by the way it started out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScrollDirection {
    /// It started out along this axis, so it only scrolls along it.
    Along(Vec2Index),
    /// It started out diagonally, so it scrolls both ways.
    Free,
}

impl ScrollDirection {
    /// The way a gesture that has traveled `travel` may go: along the axis whose compass point is nearest,
    /// or both ways if a diagonal one is. `None` until it has traveled the tap distance.
    pub fn from_travel(travel: Vec2d) -> Option<Self> {
        let (x, y) = (travel.x.abs(), travel.y.abs());
        if x.max(y) < TAP_COUNT_DISTANCE {
            return None;
        }
        // tan(22.5°), halfway between an axis and a diagonal.
        const AXIS_SLOPE: f64 = std::f64::consts::SQRT_2 - 1.0;
        Some(if y <= x * AXIS_SLOPE {
            Self::Along(Vec2Index::X)
        } else if x <= y * AXIS_SLOPE {
            Self::Along(Vec2Index::Y)
        } else {
            Self::Free
        })
    }

    /// Whether a gesture going this way may scroll along `axis`.
    pub fn allows(self, axis: Vec2Index) -> bool {
        match self {
            Self::Along(along) => along == axis,
            Self::Free => true,
        }
    }
}

/// The scroll event being dispatched.
#[derive(Clone, Copy, Debug)]
struct ScrollDispatch {
    time: f64,
    /// Per axis, whether a scrollable may claim it: a new gesture, a lost owner, or a wheel axis nothing took.
    are_claimable: [bool; 2],
    /// Whether it's momentum left over from an earlier gesture, which nothing takes.
    is_stray: bool,
}

/// The scrollable that owns the scroll gesture in progress, per axis: it takes every
/// event of the gesture and no other scrollable moves from it (see [`Cx::is_scroll_owner`]).
#[derive(Clone, Debug, Default)]
pub struct CxScrollLatch {
    gesture: ScrollGesture,
    owners: [Area; 2],
    dispatching: Option<ScrollDispatch>,
    wheel_time: f64,
    /// Per axis, how many scrollables that could take the event are passing it to their children.
    containers: [u32; 2],
    /// The way the gesture in progress may go, once it has traveled far enough to tell.
    direction: Option<ScrollDirection>,
    /// How far the gesture in progress traveled while its direction wasn't clear yet.
    travel: Vec2d,
    /// Whether the gesture in progress has moved yet. A trackpad's `Began` can carry no delta,
    /// so its first movement picks the scrollables that take it.
    has_moved: bool,
}

fn axis_slot(axis: Vec2Index) -> usize {
    match axis {
        Vec2Index::X => 0,
        Vec2Index::Y => 1,
    }
}

impl CxScrollLatch {
    pub fn owner(&self, axis: Vec2Index) -> Area {
        self.owners[axis_slot(axis)]
    }

    /// Ends the gesture, so the next scroll event starts a new one.
    pub(crate) fn reset(&mut self) {
        self.gesture = ScrollGesture::Idle;
        self.owners = [Area::Empty; 2];
    }

    /// Called before `e` is dispatched. `are_owners_live`: whether each owner is drawn and unblocked.
    pub(crate) fn begin_event(&mut self, e: &ScrollEvent, are_owners_live: [bool; 2]) {
        let is_new = match e.phase {
            ScrollPhase::Began => true,
            ScrollPhase::Changed | ScrollPhase::Ended => self.gesture != ScrollGesture::Fingers,
            ScrollPhase::None => {
                self.gesture != ScrollGesture::Wheel || e.time - self.wheel_time > WHEEL_LATCH_TIMEOUT
            }
            ScrollPhase::Momentum | ScrollPhase::MomentumEnded | ScrollPhase::Touched => false,
        };
        // Momentum belongs only to the trackpad gesture it follows.
        let is_momentum = matches!(e.phase, ScrollPhase::Momentum | ScrollPhase::MomentumEnded);
        let is_stray = is_momentum && self.gesture != ScrollGesture::Lifted;
        let mut are_claimable = [is_new; 2];
        if is_new {
            self.owners = [Area::Empty; 2];
            self.gesture = match e.phase {
                ScrollPhase::None => ScrollGesture::Wheel,
                ScrollPhase::Ended => ScrollGesture::Lifted,
                _ => ScrollGesture::Fingers,
            };
            // A wheel only turns along one axis at a time, so only a trackpad gesture keeps to the way it started.
            self.direction = (e.phase == ScrollPhase::None).then_some(ScrollDirection::Free);
            self.travel = Vec2d::default();
            self.has_moved = false;
        } else {
            if e.phase == ScrollPhase::Ended {
                self.gesture = ScrollGesture::Lifted;
            }
            let is_user_driven = matches!(e.phase, ScrollPhase::None | ScrollPhase::Changed);
            for (slot, axis) in [Vec2Index::X, Vec2Index::Y].into_iter().enumerate() {
                if self.owners[slot].is_empty() {
                    // A wheel latches only what took it, and a gesture that hasn't moved took nothing,
                    // so an axis nothing took goes to whatever can take it.
                    are_claimable[slot] = e.phase == ScrollPhase::None || !self.has_moved;
                    continue;
                }
                if is_user_driven && !are_owners_live[slot] {
                    // A removed owner frees only its own axis, for whatever is under the pointer now.
                    self.owners[slot] = Area::Empty;
                    are_claimable[slot] = true;
                } else if e.phase != ScrollPhase::Touched {
                    // Taken already, also for widgets that only check these flags.
                    e.handled(axis).set(true);
                }
            }
        }
        if e.phase == ScrollPhase::None {
            self.wheel_time = e.time;
        }
        self.has_moved |= e.scroll != Vec2d::default();
        if self.direction.is_none() {
            self.travel += e.scroll;
            self.direction = ScrollDirection::from_travel(self.travel);
        }
        self.containers = [0; 2];
        self.dispatching = Some(ScrollDispatch { time: e.time, are_claimable, is_stray });
    }

    pub(crate) fn end_event(&mut self, e: &ScrollEvent) {
        let is_stray = self.dispatching.is_some_and(|d| d.is_stray);
        self.dispatching = None;
        if e.phase == ScrollPhase::MomentumEnded && !is_stray {
            self.reset();
        }
    }

    /// Whether a wheel scroll is in progress and has rested long enough for a pointer move to end it.
    pub(crate) fn is_wheel_resting(&self, time: f64) -> bool {
        self.gesture == ScrollGesture::Wheel && time - self.wheel_time > WHEEL_LATCH_MOVE_DELAY
    }

    /// Lets go of the owner along `axis`, so the next wheel event can go to something else.
    pub(crate) fn release(&mut self, axis: Vec2Index) {
        self.owners[axis_slot(axis)] = Area::Empty;
    }

    /// Lets go of `area` if it owns `axis` but can't scroll now, so what the pointer is over can
    /// take the rest of `e`'s dispatch. Momentum stays with its owner.
    pub(crate) fn release_if_owner(&mut self, e: &ScrollEvent, area: Area, axis: Vec2Index) {
        let slot = axis_slot(axis);
        let is_user_driven = matches!(e.phase, ScrollPhase::None | ScrollPhase::Changed);
        let Some(dispatch) = &mut self.dispatching else {
            return;
        };
        if is_user_driven && dispatch.time == e.time && !area.is_empty() && self.owners[slot] == area {
            self.owners[slot] = Area::Empty;
            dispatch.are_claimable[slot] = true;
            e.handled(axis).set(false);
        }
    }

    /// See [`Cx::begin_scroll_container`].
    pub(crate) fn begin_container(&mut self, axes: [bool; 2]) {
        for slot in 0..2 {
            self.containers[slot] += axes[slot] as u32;
        }
    }

    pub(crate) fn end_container(&mut self, axes: [bool; 2]) {
        for slot in 0..2 {
            self.containers[slot] = self.containers[slot].saturating_sub(axes[slot] as u32);
        }
    }

    pub(crate) fn is_inside_container(&self, axis: Vec2Index) -> bool {
        self.containers[axis_slot(axis)] > 0
    }

    /// See [`Cx::is_scroll_owner`].
    pub(crate) fn is_owner(&mut self, e: &ScrollEvent, area: Area, axis: Vec2Index, is_candidate: bool) -> bool {
        let slot = axis_slot(axis);
        let handled = e.handled(axis);
        // A gesture two scrollers share keeps to the way it started out, like a drag does.
        let is_shared = matches!(self.owners, [x, y] if !x.is_empty() && !y.is_empty() && x != y);
        let is_locked_out = is_shared && self.direction.is_some_and(|direction| !direction.allows(axis));
        let owner = &mut self.owners[slot];
        match self.dispatching {
            Some(dispatch) if dispatch.time == e.time => {
                if dispatch.is_stray {
                    return false;
                }
                if is_locked_out {
                    handled.set(true);
                    return false;
                }
                if !owner.is_empty() {
                    return *owner == area;
                }
                // Mid-gesture, a trackpad axis nobody took once it moved stays unowned.
                if !dispatch.are_claimable[slot] || !is_candidate || handled.get() || area.is_empty() {
                    return false;
                }
                // A scroll that doesn't move claims nothing, since which way it goes decides who takes it.
                if e.scroll == Vec2d::default() {
                    return false;
                }
                *owner = area;
            }
            // An event dispatched by hand (e.g. in a test) has no gesture to follow.
            _ => {
                if e.phase == ScrollPhase::Touched || !is_candidate || handled.get() || area.is_empty() {
                    return false;
                }
            }
        }
        handled.set(true);
        true
    }

    pub(crate) fn update_area(&mut self, old_area: Area, new_area: Area) {
        for owner in &mut self.owners {
            if *owner == old_area {
                *owner = new_area;
            }
        }
    }
}

/// A trackpad pinch: macOS `magnifyWithEvent:` and Wayland's
/// `zwp_pointer_gesture_pinch_v1` (X11 and Windows report none).
///
/// `scale` is multiplicative and relative to the previous event of the same
/// gesture (1 on `Begin` and `End`), so a widget zooming about `abs` applies
/// it directly: `zoom *= scale`. Handle it through the `hit` functions as
/// [`Hit::FingerPinch`], delivered to the widget under `abs`.
#[derive(Clone, Debug)]
pub struct PinchEvent {
    pub window_id: WindowId,
    pub abs: Vec2d,
    pub scale: f64,
    pub phase: PinchPhase,
    pub modifiers: KeyModifiers,
    pub time: f64,
}

#[derive(Clone, Debug)]
pub struct LongPressEvent {
    pub window_id: WindowId,
    pub abs: Vec2d,
    pub uid: u64,
    pub time: f64,
}

// Touch events

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TouchState {
    Start,
    Stop,
    Move,
    Stable,
}

#[derive(Clone, Debug)]
pub struct TouchPoint {
    pub state: TouchState,
    pub abs: Vec2d,
    pub time: f64,
    pub uid: u64,
    pub rotation_angle: f64,
    pub force: f64,
    pub radius: Vec2d,
    pub handled: Cell<Area>,
    pub sweep_lock: Cell<Area>,
}

/// A press taken away before it lifted: its gesture was claimed by
/// another widget (a list that started scrolling under a pressed row,
/// [`CxFingers::claim_gesture`]) or by the host (a window that rotated,
/// lost focus or covered the content, [`CxFingers::cancel_digit`]).
/// Dispatched like any event; every area still holding a cancelled
/// capture of `digit_id` receives it as a terminal
/// `Hit::FingerUp` with `cancelled: true` (never over, never a tap), and
/// its capture ends there — wherever the finger is and whether or not
/// that point is still on the widget.
#[derive(Clone, Debug)]
pub struct FingerCancelEvent {
    pub window_id: WindowId,
    pub digit_id: DigitId,
    pub device: DigitDevice,
    pub abs: Vec2d,
    pub time: f64,
    pub modifiers: KeyModifiers,
}

#[derive(Clone, Debug)]
pub struct TouchUpdateEvent {
    pub time: f64,
    pub window_id: WindowId,
    pub modifiers: KeyModifiers,
    pub touches: Vec<TouchPoint>,
}

// Finger API

/// Inset represents spacing values for all four edges (left, top, right, bottom).
/// Used for both margin (outer spacing) and padding (inner spacing).
#[derive(Clone, Copy, Default, Debug, Script)]
pub struct Inset {
    /// The left inset.
    #[live]
    pub left: f64,

    /// The top inset.
    #[live]
    pub top: f64,

    /// The right inset.
    #[live]
    pub right: f64,

    /// The bottom inset.
    #[live]
    pub bottom: f64,
}

impl Inset {
    /// Returns a copy of this `Inset` with the left value set to the given value.
    pub fn with_left(mut self, left: f64) -> Self {
        Self { left, ..self }
    }

    /// Returns a copy of this `Inset` with the top value set to the given value.
    pub fn with_top(mut self, top: f64) -> Self {
        Self { top, ..self }
    }

    /// Returns a copy of this `Inset` with the right value set to the given value.
    pub fn with_right(mut self, right: f64) -> Self {
        Self { right, ..self }
    }

    /// Returns a copy of this `Inset` with the bottom value set to the given value.
    pub fn with_bottom(mut self, bottom: f64) -> Self {
        Self { bottom, ..self }
    }
}

impl Inset {
    pub fn left_top(&self) -> Vec2d {
        dvec2(self.left, self.top)
    }
    pub fn right_bottom(&self) -> Vec2d {
        dvec2(self.right, self.bottom)
    }
    pub fn size(&self) -> Vec2d {
        dvec2(self.left + self.right, self.top + self.bottom)
    }
    pub fn width(&self) -> f64 {
        self.left + self.right
    }
    pub fn height(&self) -> f64 {
        self.top + self.bottom
    }

    pub fn rect_contains_with_inset(pos: Vec2d, rect: &Rect, inset: &Option<Inset>) -> bool {
        if let Some(inset) = inset {
            return pos.x >= rect.pos.x - inset.left
                && pos.x <= rect.pos.x + rect.size.x + inset.right
                && pos.y >= rect.pos.y - inset.top
                && pos.y <= rect.pos.y + rect.size.y + inset.bottom;
        } else {
            return rect.contains(pos);
        }
    }
}

impl ScriptHook for Inset {
    fn on_type_check(_heap: &ScriptHeap, value: ScriptValue) -> bool {
        // Accept numeric values - they will set all four sides
        value.as_f64().is_some() || value.as_number().is_some()
    }

    fn on_custom_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) -> bool {
        // Handle numeric values as uniform inset
        if let Some(v) = value.as_f64() {
            *self = Inset {
                left: v,
                top: v,
                right: v,
                bottom: v,
            };
            return true;
        }
        if let Some(v) = value.as_number() {
            *self = Inset {
                left: v,
                top: v,
                right: v,
                bottom: v,
            };
            return true;
        }
        // Return false to let the generated code handle normal objects
        false
    }
}

// TODO: query the platform for its long-press timeout.
//       See Android's ViewConfiguration.getLongPressTimeout().
pub const TAP_COUNT_TIME: f64 = 0.5;
pub const TAP_COUNT_DISTANCE: f64 = 5.0;

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct DigitId(pub LiveId);

#[derive(Default, Clone)]
pub struct CxDigitCapture {
    digit_id: DigitId,
    pub(crate) has_long_press_occurred: bool,
    pub area: Area,
    pub sweep_area: Area,
    pub switch_capture: Option<Area>,
    pub time: f64,
    pub abs_start: Vec2d,
    /// A scrub pin rides ON the capture: while set, the hardware cursor is
    /// hidden+detached and pointer events exist only for this owner (hover
    /// resolution and new captures are suppressed in hits()). Because the
    /// state lives on the capture, it structurally cannot outlive the
    /// button — `mouse_up` releases the capture and the pin with it.
    pub pinned: bool,
    /// This press was taken away (another area claimed the gesture, or the
    /// host cancelled the finger): its terminal FingerUp says
    /// `cancelled`, never over, never a tap; no long press fires for it.
    pub cancelled: bool,
    /// This area claimed the finger's gesture ([`CxFingers::claim_gesture`]):
    /// the one owner of the digit until it lifts.
    pub owner: bool,
    /// The time of the event that first showed this cancelled capture to
    /// its area. Every hit test of that one event — every consumer that
    /// shares the area, in any order — sees the same terminal cancelled
    /// FingerUp; later events see nothing from it. The capture itself
    /// retires with its digit, like an ordinary release.
    cancel_seen: Option<f64>,
}

#[derive(Default, Clone)]
pub struct CxDigitTap {
    digit_id: DigitId,
    last_pos: Vec2d,
    last_time: f64,
    count: u32,
}

#[derive(Default, Clone)]
pub struct CxDigitHover {
    digit_id: DigitId,
    new_area: Area,
    area: Area,
}

/// Keeps a scoped sweep lock alive until its widget closes or is dropped.
#[must_use]
pub struct SweepLock {
    owner: Rc<Cell<bool>>,
}

impl SweepLock {
    pub fn is_active(&self) -> bool {
        self.owner.get()
    }
}

#[derive(Clone)]
struct CxSweepLock {
    area: Area,
    owner: Weak<Cell<bool>>,
}

impl CxSweepLock {
    fn is_active(&self) -> bool {
        self.owner.upgrade().is_some_and(|owner| owner.get())
    }

    fn release(&self) {
        if let Some(owner) = self.owner.upgrade() {
            owner.set(false);
        }
    }
}

#[derive(Default, Clone)]
pub struct CxFingers {
    pub first_mouse_button: Option<(MouseButton, WindowId)>,
    captures: Vec<CxDigitCapture>,
    /// Presses taken away, kept apart from the live `captures`: they own
    /// nothing, count as no capture, and block no other finger; they exist
    /// only so each area gets its one terminal cancelled FingerUp. They
    /// retire with their digit.
    cancelled: Vec<CxDigitCapture>,
    /// Fingers whose gesture was just claimed: the platform dispatches an
    /// `Event::FingerCancel` for each right after the current event, so the
    /// losers that handled that event BEFORE the claim end their press
    /// before any further input (`take_pending_cancels`).
    pending_cancels: Vec<FingerCancelEvent>,
    /// Fingers whose physical press was taken away as a whole (the host or
    /// the OS cancelled it: `cancel_digit`), until the digit is released.
    /// A claim or a recycle cancels only some captures of a press that is
    /// still held; those fingers are not here.
    taken_away: Vec<DigitId>,
    tap: CxDigitTap,
    hovers: Vec<CxDigitHover>,
    xr_poke_locks: Vec<DigitId>,
    drag_claims: Vec<DigitId>,
    /// Owners of the sweep lock, outermost first; only the LAST entry is
    /// the live lock. A single slot cannot serve NESTING OVERLAYS — a menu
    /// over a dialog over a modal — where each level takes the pointer in
    /// turn and has to hand it back to the one beneath it when it closes.
    /// With one owner this behaves exactly as a single slot does.
    sweep_locks: Vec<Area>,
    // Kept apart from stack order: modals temporarily lift and restore legacy locks.
    scoped_sweep_locks: Vec<CxSweepLock>,
    sweep_lock_validation_event: Option<u64>,
    /// Owners of the scroll block, outermost first; only the LAST entry is
    /// live. It nests for the reason the sweep lock does: a modal over a
    /// modal blocks scrolling in turn, and the one that closes has to leave
    /// the other's block standing.
    /// * While any owner is held, scrolling is blocked *except* within the
    ///   last one's area.
    /// * While the stack is empty, scrolling is not blocked anywhere.
    scroll_blocks: Vec<Area>,
    pub(crate) scroll_latch: CxScrollLatch,
    /// The scrollable that took each drag-scrolled digit along each axis.
    scroll_drag_claims: Vec<(DigitId, Vec2Index, Area)>,
    /// The drag scrollers each digit pressed, whether or not they took an axis.
    scroll_drag_holders: Vec<(DigitId, Area)>,
    /// The way each drag that two scrollers share is going.
    scroll_drag_locks: Vec<(DigitId, ScrollDirection)>,
    /// The presses something outside a drag scroller already held when it got them, which it never joins.
    presses_held_outside: Vec<(DigitId, Area)>,
}

impl CxFingers {
    /*
    pub (crate) fn get_captured_area(&self, digit_id: DigitId) -> Area {
        if let Some(cxdigit) = self.captures.iter().find( | v | v.digit_id == digit_id) {
            cxdigit.area
        }
        else {
            Area::Empty
        }
    }*/
    /*
    pub (crate) fn get_capture_time(&self, digit_id: DigitId) -> f64 {
        if let Some(cxdigit) = self.captures.iter().find( | v | v.digit_id == digit_id) {
            cxdigit.time
        }
        else {
            0.0
        }
    }*/

    /// The digit that `area` holds, if any.
    pub fn find_digit_for_captured_area(&self, area: Area) -> Option<DigitId> {
        if let Some(digit) = self.captures.iter().find(|d| d.area == area) {
            return Some(digit.digit_id);
        }
        None
    }

    pub(crate) fn update_area(&mut self, old_area: Area, new_area: Area) {
        for hover in &mut self.hovers {
            if hover.area == old_area {
                hover.area = new_area;
            }
        }
        // Cancelled presses follow their widget through a redraw too, or a
        // cancelled finger would no longer be recognised as that widget's.
        for capture in self.captures.iter_mut().chain(self.cancelled.iter_mut()) {
            if capture.area == old_area {
                capture.area = new_area;
            }
            if capture.sweep_area == old_area {
                capture.sweep_area = new_area;
            }
        }
        for lock in &mut self.sweep_locks {
            if *lock == old_area {
                *lock = new_area;
            }
        }
        for lock in &mut self.scoped_sweep_locks {
            if lock.area == old_area {
                lock.area = new_area;
                self.sweep_lock_validation_event = None;
            }
        }
        for block in &mut self.scroll_blocks {
            if *block == old_area {
                *block = new_area;
            }
        }
        self.scroll_latch.update_area(old_area, new_area);
        for (_, _, area) in &mut self.scroll_drag_claims {
            if *area == old_area {
                *area = new_area;
            }
        }
        for (_, area) in self.scroll_drag_holders.iter_mut().chain(&mut self.presses_held_outside) {
            if *area == old_area {
                *area = new_area;
            }
        }
    }

    pub(crate) fn new_hover_area(&mut self, digit_id: DigitId, new_area: Area) {
        for hover in &mut self.hovers {
            if hover.digit_id == digit_id {
                hover.new_area = new_area;
                return;
            }
        }
        self.hovers.push(CxDigitHover {
            digit_id,
            area: Area::Empty,
            new_area: new_area,
        })
    }

    pub(crate) fn find_hover_area(&self, digit: DigitId) -> Area {
        for hover in &self.hovers {
            if hover.digit_id == digit {
                return hover.area;
            }
        }
        Area::Empty
    }

    /// Whether a pointer is currently over `area`.
    pub fn is_area_hovered(&self, area: Area) -> bool {
        area != Area::Empty && self.hovers.iter().any(|hover| hover.area == area)
    }

    pub(crate) fn cycle_hover_area(&mut self, digit_id: DigitId) {
        if let Some(hover) = self.hovers.iter_mut().find(|v| v.digit_id == digit_id) {
            hover.area = hover.new_area;
            hover.new_area = Area::Empty;
        }
    }

    pub(crate) fn capture_digit(
        &mut self,
        digit_id: DigitId,
        area: Area,
        sweep_area: Area,
        time: f64,
        abs_start: Vec2d,
    ) {
        /*if let Some(capture) = self.captures.iter_mut().find( | v | v.digit_id == digit_id) {
            capture.sweep_area = sweep_area;
            capture.area = area;
            capture.time = time;
            capture.abs_start = abs_start;
        }
        else {*/
        self.captures.push(CxDigitCapture {
            sweep_area,
            digit_id,
            area,
            time,
            abs_start,
            has_long_press_occurred: false,
            switch_capture: None,
            pinned: false,
            cancelled: false,
            owner: false,
            cancel_seen: None,
        })
        /*}*/
    }

    pub(crate) fn uncapture_area(&mut self, area: Area) {
        self.captures.retain(|v| v.area != area);
    }

    pub(crate) fn find_digit_capture(&mut self, digit_id: DigitId) -> Option<&mut CxDigitCapture> {
        self.captures.iter_mut().find(|v| v.digit_id == digit_id)
    }

    pub(crate) fn find_area_capture(&mut self, area: Area) -> Option<&mut CxDigitCapture> {
        self.captures.iter_mut().find(|v| v.area == area)
    }

    /// `area`'s capture of THIS finger: a touch's move or lift resolves by
    /// its own digit, never by whichever finger holds the area.
    pub(crate) fn find_digit_area_capture(&mut self, digit_id: DigitId, area: Area) -> Option<&mut CxDigitCapture> {
        self.captures.iter_mut().find(|v| v.digit_id == digit_id && v.area == area)
    }

    /// Reassign the finger currently captured by `from` to `to` (also updating its
    /// sweep area). Lets a drag begun on one widget be handed to another mid-press.
    /// Returns true if a capture on `from` was actually found and switched — false
    /// means the finger was already released (nothing to hand off).
    pub(crate) fn switch_capture_area(&mut self, from: Area, to: Area, to_sweep: Area) -> bool {
        if let Some(cap) = self.captures.iter_mut().find(|v| v.area == from) {
            cap.area = to;
            cap.sweep_area = to_sweep;
            cap.switch_capture = None;
            true
        } else {
            false
        }
    }

    /// Hand a finger that an interactive child grabbed up to a container that
    /// already co-captures it (via `capture_overload`). Finds the digit `over`
    /// co-captures, drops every OTHER area's capture of that digit, and leaves
    /// `over` as the sole capture so it receives the finger's subsequent moves.
    /// Lets e.g. the home pager start a page-swipe/drag even when the press began
    /// on a button inside a widget tile. Returns true if a child capture was
    /// actually dropped (false = nothing was in the way).
    pub(crate) fn promote_capture_over(&mut self, over: Area) -> bool {
        let Some(digit) = self
            .captures
            .iter()
            .find(|v| v.area == over)
            .map(|v| v.digit_id)
        else {
            return false;
        };
        let before = self.captures.len();
        self.captures
            .retain(|v| v.digit_id != digit || v.area == over);
        self.captures.len() != before
    }

    /// `area` takes the gesture of the finger `digit_id` it captured (a
    /// scroller starting to scroll): it becomes the digit's one owner and
    /// every other capture of that finger is cancelled. Each loser gets a
    /// terminal cancelled FingerUp from the first event that reaches it
    /// after this — the [`Event::FingerCancel`] the owner dispatches to its
    /// children at once, or, for areas outside the owner (an ancestor
    /// scroller), the very pointer event being dispatched — and nothing
    /// from that finger after it: a loser never moves again.
    /// Returns false when the gesture is not `area`'s to take: it holds no
    /// live capture of the finger, its capture was already cancelled, or
    /// another area owns the finger — a loser must stand down.
    /// Two scrollers sharing a drag that started diagonally (see [`Self::lock_scroll_drag`])
    /// each own their axis of it, so neither cancels the other.
    pub fn claim_gesture(&mut self, digit_id: DigitId, area: Area) -> bool {
        let Some(own) = self.captures.iter().position(|v| v.digit_id == digit_id && v.area == area) else {
            // No live capture: none, or already cancelled.
            return false;
        };
        if self.captures[own].owner {
            return true;
        }
        let partner = match [Vec2Index::X, Vec2Index::Y].map(|axis| self.scroll_drag_owner(digit_id, axis)) {
            [Some(x), Some(y)]
                if x != y
                    && (x == area || y == area)
                    && self.scroll_drag_locks.contains(&(digit_id, ScrollDirection::Free)) =>
            {
                Some(if x == area { y } else { x })
            }
            _ => None,
        };
        if let Some(owner) = self.captures.iter().find(|v| v.digit_id == digit_id && v.owner) {
            if Some(owner.area) == partner {
                self.captures[own].owner = true;
                return true;
            }
            let lost = self.captures.remove(own);
            self.cancelled.push(CxDigitCapture { cancelled: true, ..lost });
            return false;
        }
        self.captures[own].owner = true;
        let before = self.cancelled.len();
        self.move_to_cancelled(|v| v.digit_id == digit_id && !v.owner && Some(v.area) != partner);
        if self.cancelled.len() > before {
            let abs = self.captures.iter().find(|v| v.digit_id == digit_id).map(|v| v.abs_start).unwrap_or_default();
            self.queue_cancel(digit_id, abs);
        }
        true
    }

    /// The host takes the finger `digit_id` away entirely (its window
    /// rotated, lost focus, or the content it pressed left the front):
    /// every capture of it is cancelled. The host then dispatches
    /// [`Event::FingerCancel`] so each widget ends its press, and retires
    /// the digit.
    pub fn cancel_digit(&mut self, digit_id: DigitId) {
        self.move_to_cancelled(|v| v.digit_id == digit_id);
        if !self.taken_away.contains(&digit_id) {
            self.taken_away.push(digit_id);
        }
    }

    /// Whether the press of `digit_id` itself was taken away (`cancel_digit`:
    /// the host rotated, the OS cancelled the touch) — as opposed to a
    /// `FingerCancel` that ends only some captures of a press still held (a
    /// list took the finger, a pressed row was recycled). Widgets that follow
    /// the raw pointer end their own gesture only on this: its release will
    /// not come.
    pub fn press_taken_away(&self, digit_id: DigitId) -> bool {
        self.taken_away.contains(&digit_id)
    }

    /// A container is about to drop, hide or recycle what a live press of
    /// `digit_id` landed on (a list recycling a pressed row): every capture of
    /// that finger except the container's own `keep` is cancelled, and a
    /// `FingerCancel` is queued so each ends its press right after the
    /// current event, wherever it now lives.
    pub fn cancel_digit_except(&mut self, digit_id: DigitId, keep: Area) {
        let before = self.cancelled.len();
        self.move_to_cancelled(|v| v.digit_id == digit_id && v.area != keep);
        if self.cancelled.len() > before {
            let abs = self.cancelled.last().map(|v| v.abs_start).unwrap_or_default();
            self.queue_cancel(digit_id, abs);
        }
    }

    fn move_to_cancelled(&mut self, which: impl Fn(&CxDigitCapture) -> bool) {
        let mut index = 0;
        while index < self.captures.len() {
            if which(&self.captures[index]) {
                let capture = self.captures.remove(index);
                self.cancelled.push(CxDigitCapture { cancelled: true, owner: false, cancel_seen: None, ..capture });
            } else {
                index += 1;
            }
        }
    }

    fn queue_cancel(&mut self, digit_id: DigitId, abs: Vec2d) {
        if self.pending_cancels.iter().any(|e| e.digit_id == digit_id) {
            return;
        }
        let device = if digit_id == live_id!(mouse).into() {
            DigitDevice::Mouse { button: MouseButton::PRIMARY }
        } else {
            DigitDevice::Touch { uid: 0 }
        };
        self.pending_cancels.push(FingerCancelEvent {
            window_id: WindowId(0, 0),
            digit_id,
            device,
            abs,
            time: 0.0,
            modifiers: KeyModifiers::default(),
        });
    }

    /// The cancellations a claim queued during the event just dispatched,
    /// stamped `now`: the platform dispatches each as `Event::FingerCancel`
    /// before any further input. A test harness driving widgets directly
    /// does the same after each event.
    pub fn take_pending_cancels(&mut self, now: f64) -> Vec<FingerCancelEvent> {
        let mut pending = std::mem::take(&mut self.pending_cancels);
        for event in &mut pending {
            event.time = now;
        }
        pending
    }

    /// The terminal hit of a cancelled capture, if `area` holds one of
    /// `digit_id`: `Some(true)` while the event at `time` is the one that
    /// shows the cancel (every consumer of that event sees it), `Some(false)`
    /// for any later event (nothing more from this finger), `None` when the
    /// capture is not cancelled (ordinary handling).
    fn cancelled_capture(&mut self, digit_id: DigitId, area: Area, time: f64) -> Option<(bool, CxDigitCapture)> {
        let capture = self.cancelled.iter_mut().find(|v| v.digit_id == digit_id && v.area == area)?;
        let seen = *capture.cancel_seen.get_or_insert(time);
        Some((seen == time, capture.clone()))
    }

    /// Whether a press is captured by `area`. A press parked on `Area::Empty` (e.g., between sweeps)
    /// isn't captured by every area that hasn't been drawn yet.
    pub fn is_area_captured(&self, area: Area) -> bool {
        area != Area::Empty && self.captures.iter().any(|v| v.area == area)
    }

    /// Whether `digit_id` is held by an area other than `area`, so a
    /// capture-overload hit can tell a press a child already owns.
    pub fn is_digit_captured_elsewhere(&self, digit_id: DigitId, area: Area) -> bool {
        self.captures.iter().any(|v| v.digit_id == digit_id && v.area != area)
    }

    /// The area that captured the touch with the given uid, if any.
    /// Lets a raw `Event::LongPress` handler check which widget owns the press.
    pub fn touch_capture_area(&self, uid: u64) -> Option<Area> {
        let digit_id: DigitId = live_id_num!(touch, uid).into();
        self.captures.iter().find(|v| v.digit_id == digit_id).map(|v| v.area)
    }

    pub(crate) fn claim_drag(&mut self, digit_id: DigitId) {
        if !self.drag_claims.contains(&digit_id) {
            self.drag_claims.push(digit_id);
        }
    }

    /// Whether a widget claimed the drag of the touch with the given uid (see [`Cx::claim_finger_drag`])
    /// or already owns its gesture (see [`Cx::claim_finger_gesture`]), so a container shouldn't make it its own gesture.
    pub fn is_touch_drag_claimed(&self, uid: u64) -> bool {
        self.is_drag_claimed(live_id_num!(touch, uid).into())
    }

    /// Like [`Self::is_touch_drag_claimed`], for any digit (a mouse press too).
    pub fn is_drag_claimed(&self, digit_id: DigitId) -> bool {
        self.is_drag_claimed_by_widget(digit_id)
            || self.captures.iter().any(|v| v.digit_id == digit_id && v.owner)
    }

    /// Whether a widget claimed `digit_id`'s drag (see [`Cx::claim_finger_drag`]), not counting
    /// a scroller that owns its gesture.
    pub fn is_drag_claimed_by_widget(&self, digit_id: DigitId) -> bool {
        self.drag_claims.contains(&digit_id)
    }

    pub fn any_areas_captured(&self) -> bool {
        self.captures.len() > 0
    }

    /// Every capture of the MOUSE right now. Normally at most one, but a
    /// container may co-capture a press a child already took, so one press
    /// can have more than one owner.
    fn mouse_captures(&self) -> impl Iterator<Item = &CxDigitCapture> {
        let digit_id: DigitId = live_id!(mouse).into();
        self.captures.iter().filter(move |c| c.digit_id == digit_id)
    }

    /// Is the mouse currently held by something that is NOT one of `mine`?
    ///
    /// Ask this from a gesture that starts on a RAW press (a list's
    /// drag-to-scroll, a canvas pan, a rubber-band select) — code that never
    /// goes through `Event::hits` and so never learns that another widget
    /// took the press. Pass the areas the asking host owns (its own area,
    /// its scroll bars); `true` means another control owns the pointer and
    /// the gesture must stand down.
    ///
    /// # The rule this exists for
    ///
    /// Every control that is dragged continuously — a slider, a scrollbar, a
    /// fader, a resizer, a long-press button — takes pointer capture on the
    /// press. From then until the release the interaction is LOCKED to that
    /// control: it keeps tracking the pointer outside its own bounds, and no
    /// other element may take a hover, focus or press-like state from that
    /// pointer on the way. A reorder carry, a scroll grab, or any other
    /// gesture that would start from the same press stands down while
    /// another control holds the mouse. The one exception is drag and drop:
    /// there the source does NOT lock the pointer, because a global drag
    /// state must let other components light their drag-over states and
    /// accept the drop on release.
    ///
    /// `hits()` already implements the captured half: a press captures the
    /// digit, and while a button is down no other area is handed hovers.
    /// This is the other half — what a raw-press gesture has to ask for
    /// itself, at the press AND on every move while it is still pending,
    /// dropping itself the moment the answer is yes. The press and the
    /// child's capture can land in either order within one event, so a raw
    /// gesture must not rely on this alone at press time: it may only take a
    /// press on BARE BACKGROUND, i.e. when no widget but the host itself is
    /// found under the point, whether or not that widget captured.
    ///
    /// Only the mouse locks: a TOUCH capture elsewhere answers `false`,
    /// because a touch drag that begins on a control is still allowed to
    /// scroll the list under it.
    ///
    /// Areas are matched by owner, not by handle: a redraw hands the caller
    /// a fresh `Area` for the same widget (a new `redraw_id`) while the
    /// capture still records the one taken at the press, and that is still
    /// `mine`.
    pub fn is_mouse_held_outside(&self, mine: &[Area]) -> bool {
        self.mouse_captures()
            .any(|c| !mine.iter().any(|m| same_owner(*m, c.area)))
    }

    /// Like [`Self::is_mouse_held_outside`], for a drag scroller: another drag scroller holding
    /// the press doesn't count, since nested scrollers split a drag by axis.
    pub fn is_mouse_held_outside_scrollers(&self, mine: &[Area]) -> bool {
        let digit_id: DigitId = live_id!(mouse).into();
        self.mouse_captures().any(|c| {
            !mine.iter().any(|m| same_owner(*m, c.area)) && !self.is_scroll_drag_holder(digit_id, c.area)
        })
    }

    pub(crate) fn release_digit(&mut self, digit_id: DigitId) {
        self.cancelled.retain(|v| v.digit_id != digit_id);
        self.taken_away.retain(|d| *d != digit_id);
        while let Some(index) = self
            .captures
            .iter_mut()
            .position(|v| v.digit_id == digit_id)
        {
            self.captures.remove(index);
        }
        self.drag_claims.retain(|id| *id != digit_id);
        self.scroll_drag_claims.retain(|(id, ..)| *id != digit_id);
        self.scroll_drag_holders.retain(|(id, _)| *id != digit_id);
        self.scroll_drag_locks.retain(|(id, _)| *id != digit_id);
        self.presses_held_outside.retain(|(id, _)| *id != digit_id);
    }

    /// Whether `area` owns `digit_id`'s drag along `axis`, taking it unless another
    /// scrollable (a nested one, dispatched first) already did.
    pub(crate) fn is_scroll_drag_owner(&mut self, digit_id: DigitId, axis: Vec2Index, area: Area) -> bool {
        match self.scroll_drag_owner(digit_id, axis) {
            Some(owner) => owner == area,
            None => {
                self.scroll_drag_claims.push((digit_id, axis, area));
                true
            }
        }
    }

    /// The scrollable that took `digit_id`'s drag along `axis`, if any.
    pub fn scroll_drag_owner(&self, digit_id: DigitId, axis: Vec2Index) -> Option<Area> {
        self.scroll_drag_claims
            .iter()
            .find(|(id, a, _)| *id == digit_id && *a == axis)
            .map(|(_, _, area)| *area)
    }

    /// See [`Cx::hold_scroll_drag`].
    pub(crate) fn hold_scroll_drag(&mut self, digit_id: DigitId, area: Area) {
        if !self.scroll_drag_holders.contains(&(digit_id, area)) {
            self.scroll_drag_holders.push((digit_id, area));
        }
    }

    /// Whether `area` is a drag scroller holding `digit_id`'s press, or took an axis of its drag.
    fn is_scroll_drag_holder(&self, digit_id: DigitId, area: Area) -> bool {
        self.scroll_drag_holders.contains(&(digit_id, area))
            || self.scroll_drag_claims.iter().any(|(id, _, a)| *id == digit_id && *a == area)
    }

    /// Whether `area` can join the drag scrollers holding `digit_id`'s press, to drag it along `axis`:
    /// only drag scrollers inside `area` hold it, none of them took `axis`, and no widget claimed the drag.
    pub fn can_join_scroll_drag(&self, digit_id: DigitId, axis: Vec2Index, area: Area) -> bool {
        let mut holders = self
            .captures
            .iter()
            .filter(|capture| capture.digit_id == digit_id && capture.area != area)
            .peekable();
        holders.peek().is_some()
            && !self.is_drag_claimed(digit_id)
            && !self.presses_held_outside.contains(&(digit_id, area))
            && self.scroll_drag_owner(digit_id, axis).is_none_or(|owner| owner == area)
            && holders.all(|capture| self.is_scroll_drag_holder(digit_id, capture.area))
    }

    /// Marks `digit_id`'s press as already held by something outside `area` (like an overlay drawn
    /// over it), so `area` never joins it. Call it before passing the press to what's inside `area`.
    pub fn mark_press_as_held_outside(&mut self, digit_id: DigitId, area: Area) {
        if !self.presses_held_outside.contains(&(digit_id, area)) {
            self.presses_held_outside.push((digit_id, area));
        }
    }

    /// Whether two scrollers took `digit_id`'s drag, one along each axis.
    pub fn is_scroll_drag_shared(&self, digit_id: DigitId) -> bool {
        let owners = [Vec2Index::X, Vec2Index::Y].map(|axis| self.scroll_drag_owner(digit_id, axis));
        matches!(owners, [Some(x), Some(y)] if x != y)
    }

    /// See [`Cx::lock_scroll_drag`].
    pub(crate) fn lock_scroll_drag(&mut self, digit_id: DigitId, travel: Vec2d) -> Option<ScrollDirection> {
        if !self.is_scroll_drag_shared(digit_id) {
            return Some(ScrollDirection::Free);
        }
        if let Some((_, direction)) = self.scroll_drag_locks.iter().find(|(id, _)| *id == digit_id) {
            return Some(*direction);
        }
        let direction = ScrollDirection::from_travel(travel)?;
        self.scroll_drag_locks.push((digit_id, direction));
        Some(direction)
    }

    pub(crate) fn remove_hover(&mut self, digit_id: DigitId) {
        while let Some(index) = self.hovers.iter_mut().position(|v| v.digit_id == digit_id) {
            self.hovers.remove(index);
        }
    }

    pub(crate) fn xr_poke_is_locked(&self, digit_id: DigitId) -> bool {
        self.xr_poke_locks.contains(&digit_id)
    }

    pub(crate) fn xr_poke_lock(&mut self, digit_id: DigitId) {
        if !self.xr_poke_locks.contains(&digit_id) {
            self.xr_poke_locks.push(digit_id);
        }
    }

    pub(crate) fn xr_poke_unlock(&mut self, digit_id: DigitId) {
        self.xr_poke_locks.retain(|id| *id != digit_id);
    }

    pub(crate) fn tap_count(&self) -> u32 {
        self.tap.count
    }

    pub(crate) fn process_tap_count(&mut self, pos: Vec2d, time: f64) -> u32 {
        // TODO: query the platform for its multi-press / double-click timeout.
        //       e.g., see Android's ViewConfiguration.getMultiPressTimeout().
        if (time - self.tap.last_time) < TAP_COUNT_TIME
            && pos.distance(&self.tap.last_pos) < TAP_COUNT_DISTANCE
        {
            self.tap.count += 1;
            // Cycle back after triple-click so fast repeated
            // double-clicks keep working (1→2→3→1→2→3…).
            if self.tap.count > 3 {
                self.tap.count = 1;
            }
        } else {
            self.tap.count = 1;
        }
        self.tap.last_pos = pos;
        self.tap.last_time = time;
        return self.tap.count;
    }

    pub(crate) fn process_touch_update_start(&mut self, time: f64, touches: &[TouchPoint]) {
        for touch in touches {
            if let TouchState::Start = touch.state {
                self.process_tap_count(touch.abs, time);
            }
        }
    }

    /// Bookkeeping after a `TouchUpdate` was dispatched: a lifted finger
    /// releases its capture and hover, the others cycle their hover. The
    /// platform backends call it for real touches; a host that synthesizes
    /// a touch for content it embeds (a phone simulator presenting the
    /// mouse as a finger — its tap count is the mouse press's own) calls it
    /// after each dispatch, and on a cancel whether or not anything is left
    /// to dispatch to.
    pub fn process_touch_update_end(&mut self, touches: &[TouchPoint]) {
        for touch in touches {
            let digit_id = live_id_num!(touch, touch.uid).into();
            match touch.state {
                TouchState::Stop => {
                    self.release_digit(digit_id);
                    self.remove_hover(digit_id);
                }
                TouchState::Start | TouchState::Move | TouchState::Stable => {
                    self.cycle_hover_area(digit_id);
                }
            }
        }
        self.switch_captures();
    }

    pub(crate) fn mouse_down(&mut self, button: MouseButton, window_id: WindowId) {
        if self.first_mouse_button.is_none() {
            self.first_mouse_button = Some((button, window_id));
        }
    }

    pub(crate) fn switch_captures(&mut self) {
        for capture in &mut self.captures {
            if let Some(area) = capture.switch_capture {
                capture.area = area;
                capture.switch_capture = None;
            }
        }
    }

    pub(crate) fn mouse_up(&mut self, button: MouseButton) {
        match self.first_mouse_button {
            Some((fmb, _)) if fmb == button => {
                self.first_mouse_button = None;
                let digit_id = live_id!(mouse).into();
                self.release_digit(digit_id);
            }
            _ => {}
        }
    }

    /// True while any capture carries a scrub pin (the owner is the only
    /// consumer of pointer events).
    pub fn has_pinned_capture(&self) -> bool {
        self.captures.iter().any(|c| c.pinned)
    }

    /// Mark the current mouse capture pinned. Returns false when there is
    /// no mouse capture to pin (the press was already released).
    pub(crate) fn pin_mouse_capture(&mut self) -> bool {
        let digit_id = live_id!(mouse).into();
        if let Some(c) = self.captures.iter_mut().find(|c| c.digit_id == digit_id) {
            c.pinned = true;
            true
        } else {
            false
        }
    }

    /// Clear the pin flag on whatever capture carries it (release or
    /// cancel). Returns true when a pin was actually cleared.
    pub(crate) fn unpin_captures(&mut self) -> bool {
        let mut any = false;
        for c in &mut self.captures {
            if c.pinned {
                c.pinned = false;
                any = true;
            }
        }
        any
    }

    pub(crate) fn test_sweep_lock(&mut self, sweep_area: Area) -> bool {
        self.clear_expired_sweep_locks();
        if let Some(lock) = self.sweep_lock_area() {
            if lock != sweep_area {
                return true;
            }
        }
        false
    }

    /// Take the sweep lock for `area`, on top of any lock already held. An
    /// owner already in the stack keeps its one entry and its level, so
    /// taking the lock twice is not two owners.
    pub fn sweep_lock(&mut self, area: Area) {
        self.clear_expired_sweep_locks();
        if !self.sweep_locks.contains(&area) {
            self.sweep_locks.push(area);
        }
    }

    /// Take a scoped lock above existing owners, unless this area already owns one.
    pub fn acquire_sweep_lock(&mut self, area: Area) -> Option<SweepLock> {
        self.clear_expired_sweep_locks();
        if self.sweep_locks.contains(&area)
            || self.scoped_sweep_locks.iter().any(|lock| lock.area == area)
        {
            return None;
        }
        let owner = Rc::new(Cell::new(true));
        self.sweep_locks.push(area);
        self.scoped_sweep_locks.push(CxSweepLock { area, owner: Rc::downgrade(&owner) });
        self.sweep_lock_validation_event = None;
        Some(SweepLock { owner })
    }

    fn clear_expired_sweep_locks(&mut self) {
        self.sweep_locks.retain(|area| !self.scoped_sweep_locks.iter()
            .any(|lock| lock.area == *area && !lock.is_active()));
        self.scoped_sweep_locks.retain(CxSweepLock::is_active);
    }

    /// The area holding the sweep lock right now — the innermost of the
    /// owners pushed by [`Self::sweep_lock`] — if any.
    ///
    /// A popover that grabs the pointer (a drop-down, a radial menu) reads
    /// this to tell "I hold it" from "somebody above me holds it", so it
    /// releases only its own grab.
    pub fn sweep_lock_area(&self) -> Option<Area> {
        self.sweep_locks.iter().rev().find(|area| !self.scoped_sweep_locks.iter()
            .any(|lock| lock.area == **area && !lock.is_active())).copied()
    }

    /// Release `area`'s sweep lock wherever it sits in the stack. Letting go
    /// of an outer owner while an inner one is held leaves the inner one on
    /// top; an area that holds no lock is a no-op.
    pub fn sweep_unlock(&mut self, area: Area) {
        self.sweep_locks.retain(|lock| *lock != area);
    }

    /// Returns the excepted area in which scrolling is currently allowed:
    /// the innermost owner's.
    /// * If `Some`, scrolling is currently blocked *except* within the contained area.
    /// * If `None`, scrolling is not blocked anywhere.
    pub fn blocked_scrolling_exception_area(&self) -> Option<Area> {
        self.scroll_blocks.last().copied()
    }

    /// `Some(area)` blocks scrolling everywhere but within `area`, on top of
    /// any block already held. An owner already in the stack keeps its one
    /// entry and its level — blocks are re-asserted on every draw, with a
    /// fresh handle each time, so they are matched by owner. `None` releases
    /// the innermost block only.
    pub fn block_scrolling_within_area(&mut self, area: Option<Area>) {
        match area {
            Some(area) => match self.scroll_blocks.iter_mut().find(|block| same_owner(**block, area)) {
                Some(block) => *block = area,
                None => self.scroll_blocks.push(area),
            },
            None => {
                self.scroll_blocks.pop();
            }
        }
    }

    /// Release the scroll block owned by `area` wherever it sits in the
    /// stack, leaving a block another owner holds in place.
    ///
    /// Unlike passing `None` to [`Self::block_scrolling_within_area`], this
    /// is safe to call from a widget that does not know whether its own
    /// block is still the current one — a panel closing after something
    /// else opened over it clears nothing.
    pub fn unblock_scrolling_within_area(&mut self, area: Area) {
        self.scroll_blocks.retain(|block| !same_owner(*block, area));
    }
}

/// Two areas name the same lock owner when they address the same rect or
/// the same instance run on the same draw list. The `redraw_id` a draw
/// stamps on an area is left out on purpose: an owner that re-asserts its
/// lock after every draw hands over a fresh handle each time, and it must
/// still find its own entry.
fn same_owner(a: Area, b: Area) -> bool {
    match (a, b) {
        (Area::Instance(a), Area::Instance(b)) => {
            a.draw_list_id == b.draw_list_id
                && a.draw_item_id == b.draw_item_id
                && a.instance_offset == b.instance_offset
        }
        (Area::Rect(a), Area::Rect(b)) => a.draw_list_id == b.draw_list_id && a.rect_id == b.rect_id,
        (Area::Empty, Area::Empty) => true,
        _ => false,
    }
}

#[derive(Clone, Debug)]
pub enum DigitDevice {
    Mouse { button: MouseButton },
    Touch { uid: u64 },
    XrHand { is_left: bool, index: usize },
    XrController {},
}

impl DigitDevice {
    /// Returns true if this device is a touch device.
    pub fn is_touch(&self) -> bool {
        matches!(self, Self::Touch { .. })
    }
    /// Returns true if this device is a mouse.
    pub fn is_mouse(&self) -> bool {
        matches!(self, Self::Mouse { .. })
    }
    /// Returns true if this device is an XR device.
    pub fn is_xr_hand(&self) -> bool {
        matches!(self, Self::XrHand { .. })
    }
    pub fn is_xr_controller(&self) -> bool {
        matches!(self, Self::XrController { .. })
    }
    /// Returns true if this device can hover: either a mouse or an XR device.
    pub fn has_hovers(&self) -> bool {
        matches!(
            self,
            Self::Mouse { .. } | Self::XrController { .. } | Self::XrHand { .. }
        )
    }
    /// Returns the `MouseButton` if this device is a mouse; otherwise `None`.
    pub fn mouse_button(&self) -> Option<MouseButton> {
        if let Self::Mouse { button } = self {
            Some(*button)
        } else {
            None
        }
    }
    /// Returns the `uid` of the touch device if this device is a touch device; otherwise `None`.
    pub fn touch_uid(&self) -> Option<u64> {
        if let Self::Touch { uid } = self {
            Some(*uid)
        } else {
            None
        }
    }
    /// Returns true if this is a *primary* mouse button hit *or* any touch hit.
    pub fn is_primary_hit(&self) -> bool {
        match self {
            DigitDevice::Mouse { button } => button.is_primary(),
            DigitDevice::Touch { .. } => true,
            DigitDevice::XrHand { .. } => true,
            DigitDevice::XrController { .. } => true,
        }
    }
    // pub fn xr_input(&self) -> Option<usize> {if let DigitDevice::XR(input) = self {Some(*input)}else {None}}
}

#[derive(Clone, Debug)]
pub struct FingerDownEvent {
    pub window_id: WindowId,
    pub abs: Vec2d,

    pub digit_id: DigitId,
    pub device: DigitDevice,

    pub tap_count: u32,
    pub modifiers: KeyModifiers,
    pub time: f64,
    pub rect: Rect,
}
impl Deref for FingerDownEvent {
    type Target = DigitDevice;
    fn deref(&self) -> &DigitDevice {
        &self.device
    }
}
impl FingerDownEvent {
    pub fn mod_control(&self) -> bool {
        self.modifiers.control
    }
    pub fn mod_alt(&self) -> bool {
        self.modifiers.alt
    }
    pub fn mod_shift(&self) -> bool {
        self.modifiers.shift
    }
    pub fn mod_logo(&self) -> bool {
        self.modifiers.logo
    }
}

#[derive(Clone, Debug)]
pub struct FingerMoveEvent {
    pub window_id: WindowId,
    pub abs: Vec2d,
    pub digit_id: DigitId,
    pub device: DigitDevice,
    /// Whether a platform-native long press has occurred between
    /// the original finger-down event and this finger-move event.
    pub has_long_press_occurred: bool,

    pub tap_count: u32,
    pub modifiers: KeyModifiers,
    pub time: f64,

    pub abs_start: Vec2d,
    pub rect: Rect,
    pub is_over: bool,
}
impl Deref for FingerMoveEvent {
    type Target = DigitDevice;
    fn deref(&self) -> &DigitDevice {
        &self.device
    }
}
impl FingerMoveEvent {
    pub fn move_distance(&self) -> f64 {
        ((self.abs_start.x - self.abs.x).powf(2.) + (self.abs_start.y - self.abs.y).powf(2.)).sqrt()
    }
}

#[derive(Clone, Debug)]
pub struct FingerUpEvent {
    pub window_id: WindowId,
    /// The absolute position of this finger-up event.
    pub abs: Vec2d,
    /// The absolute position of the original finger-down event.
    pub abs_start: Vec2d,
    /// The time at which the original finger-down event occurred.
    pub capture_time: f64,
    /// The time at which this finger-up event occurred.
    pub time: f64,

    pub digit_id: DigitId,
    pub device: DigitDevice,
    /// Whether a platform-native long press has occurred between
    /// the original finger-down event and this finger-up event.
    pub has_long_press_occurred: bool,

    pub tap_count: u32,
    pub modifiers: KeyModifiers,
    pub rect: Rect,
    /// Whether this finger-up event (`abs`) occurred within the hits area.
    pub is_over: bool,
    pub is_sweep: bool,
    /// The press was taken away rather than released (see
    /// [`FingerCancelEvent`]): end the press — let go of a pressed look,
    /// stop a drag without flinging — but activate nothing. Never over,
    /// never a tap.
    pub cancelled: bool,
}
impl Deref for FingerUpEvent {
    type Target = DigitDevice;
    fn deref(&self) -> &DigitDevice {
        &self.device
    }
}
impl FingerUpEvent {
    /// Returns `true` if this FingerUp event was a regular tap/click (not a long press).
    pub fn was_tap(&self) -> bool {
        if self.cancelled || self.has_long_press_occurred {
            return false;
        }
        self.time - self.capture_time < TAP_COUNT_TIME
            && (self.abs_start - self.abs).length() < TAP_COUNT_DISTANCE
    }
}

#[derive(Clone, Debug)]
pub struct FingerLongPressEvent {
    pub window_id: WindowId,
    /// The absolute position of this long-press event.
    pub abs: Vec2d,
    /// The time at which the original finger-down event occurred.
    pub capture_time: f64,
    /// The time at which this long-press event occurred.
    pub time: f64,

    pub digit_id: DigitId,
    pub device: DigitDevice,
    pub rect: Rect,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum HoverState {
    In,
    #[default]
    Over,
    Out,
}

#[derive(Clone, Debug)]
pub struct FingerHoverEvent {
    pub window_id: WindowId,
    pub abs: Vec2d,
    pub digit_id: DigitId,
    pub device: DigitDevice,
    pub modifiers: KeyModifiers,
    pub time: f64,
    pub rect: Rect,
}

#[derive(Clone, Debug)]
pub struct FingerScrollEvent {
    pub window_id: WindowId,
    pub digit_id: DigitId,
    pub abs: Vec2d,
    pub scroll: Vec2d,
    pub device: DigitDevice,
    pub modifiers: KeyModifiers,
    pub time: f64,
    pub rect: Rect,
    pub phase: ScrollPhase,
}

#[derive(Clone, Debug)]
pub struct FingerPinchEvent {
    pub window_id: WindowId,
    pub abs: Vec2d,
    /// The change since the previous event of the gesture (see [`PinchEvent`]).
    pub scale: f64,
    pub phase: PinchPhase,
    pub modifiers: KeyModifiers,
    pub time: f64,
    pub rect: Rect,
}

/*
pub enum HitTouch {
    Single,
    Multi
}*/

// Status

#[derive(Clone, Debug, Default)]
pub struct HitOptions {
    pub margin: Option<Inset>,
    /// Hit-test margin to use when the event came from a touch device.
    /// Falls back to `margin` when `None`. Lets a widget widen its grab area
    /// for fingers without enlarging the mouse hit zone — useful on hybrid
    /// devices (e.g. touchscreen Windows/Linux laptops) where compile-time
    /// platform detection isn't enough.
    pub touch_margin: Option<Inset>,
    pub sweep_area: Area,
    pub capture_overload: bool,
}

impl HitOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_sweep_area(self, area: Area) -> Self {
        Self {
            sweep_area: area,
            ..self
        }
    }
    pub fn with_margin(self, margin: Inset) -> Self {
        Self {
            margin: Some(margin),
            ..self
        }
    }
    pub fn with_touch_margin(self, margin: Inset) -> Self {
        Self {
            touch_margin: Some(margin),
            ..self
        }
    }
    pub fn with_capture_overload(self, capture_overload: bool) -> Self {
        Self {
            capture_overload,
            ..self
        }
    }

    /// Returns the margin to apply for a hit-test against `device`.
    /// Touch devices get `touch_margin` if set; everything else uses `margin`.
    fn margin_for(&self, device: &DigitDevice) -> Option<Inset> {
        if device.is_touch() && self.touch_margin.is_some() {
            self.touch_margin
        } else {
            self.margin
        }
    }
}

impl Cx {
    pub(crate) fn validate_scoped_sweep_locks_after_draw(&mut self) {
        self.fingers.sweep_lock_validation_event = None;
        self.validate_scoped_sweep_locks();
    }

    fn validate_scoped_sweep_locks(&mut self) {
        self.fingers.clear_expired_sweep_locks();
        let event_id = self.event_id();
        if self.fingers.scoped_sweep_locks.is_empty()
            || self.fingers.sweep_lock_validation_event == Some(event_id)
        {
            return;
        }
        self.fingers.sweep_lock_validation_event = Some(event_id);
        for lock in &self.fingers.scoped_sweep_locks {
            let area = lock.area;
            let attached = area.is_valid(self)
                && area.draw_list_id()
                    .and_then(|list| self.draw_lists[list].draw_pass_id)
                    .is_some_and(|pass| !self.pass_attachment_is_stale(pass)
                        && area.is_attached(self, &self.attached_draw_lists(pass)));
            if !attached {
                lock.release();
            }
        }
        self.fingers.clear_expired_sweep_locks();
    }
}

impl Event {
    /// `area`'s answer for a pointer event that meets one of its CANCELLED
    /// captures: the terminal `FingerUp { cancelled: true }` for the one
    /// event that shows the cancel, `Hit::Nothing` for every event after it
    /// (a cancelled press never moves, taps or long-presses again). `None`
    /// when this event holds no cancelled capture of `area`.
    fn cancelled_press_hit(&self, cx: &mut Cx, area: Area) -> Option<Hit> {
        let (digit_id, device, abs, time, window_id, modifiers) = match self {
            Event::FingerCancel(e) => (e.digit_id, e.device.clone(), e.abs, e.time, e.window_id, e.modifiers),
            Event::TouchUpdate(e) => {
                // Only a cancelled finger whose terminal is due in THIS event
                // answers here; a finger already told is skipped by the
                // ordinary per-touch loop, so it never hides another finger
                // of the same batch.
                let t = e.touches.iter().find(|t| {
                    let digit: DigitId = live_id_num!(touch, t.uid).into();
                    cx.fingers.cancelled.iter().any(|c| {
                        c.digit_id == digit && c.area == area && c.cancel_seen.map_or(true, |seen| seen == e.time)
                    })
                })?;
                (live_id_num!(touch, t.uid).into(), DigitDevice::Touch { uid: t.uid }, t.abs, e.time, e.window_id, e.modifiers)
            }
            Event::LongPress(e) => {
                let digit: DigitId = live_id_num!(touch, e.uid).into();
                cx.fingers.cancelled.iter().any(|c| c.digit_id == digit && c.area == area).then_some(())?;
                return Some(Hit::Nothing);
            }
            Event::MouseMove(e) => (live_id!(mouse).into(), DigitDevice::Mouse { button: MouseButton::PRIMARY }, e.abs, e.time, e.window_id, e.modifiers),
            Event::MouseUp(e) => (live_id!(mouse).into(), DigitDevice::Mouse { button: e.button }, e.abs, e.time, e.window_id, e.modifiers),
            _ => return None,
        };
        let (terminal, capture) = cx.fingers.cancelled_capture(digit_id, area, time)?;
        if !terminal {
            return Some(Hit::Nothing);
        }
        let rect = if area.is_valid(cx) { area.clipped_rect(cx) } else { Rect::default() };
        Some(Hit::FingerUp(FingerUpEvent {
            window_id,
            abs,
            abs_start: capture.abs_start,
            capture_time: capture.time,
            time,
            digit_id,
            device,
            has_long_press_occurred: capture.has_long_press_occurred,
            tap_count: cx.fingers.tap_count(),
            modifiers,
            rect,
            is_over: false,
            is_sweep: false,
            cancelled: true,
        }))
    }

    pub fn unhandle(&self, cx: &mut Cx, area: &Area) {
        match self {
            Event::TouchUpdate(e) => {
                for t in &e.touches {
                    if let TouchState::Start = t.state {
                        if t.handled.get() == *area {
                            t.handled.set(Area::Empty);
                        }
                        cx.fingers.uncapture_area(*area);
                    }
                }
            }
            Event::MouseDown(fd) => {
                if fd.handled.get() == *area {
                    fd.handled.set(Area::Empty);
                }
                cx.fingers.uncapture_area(*area);
            }
            _ => (),
        }
    }

    /// The area that has claimed this pointer event so far (hover, mouse press, or touch start).
    /// Snapshot it before dispatching a subtree: a claim appearing across that dispatch came from within.
    pub fn pointer_claimed_area(&self) -> Area {
        match self {
            Event::MouseMove(e) => e.handled.get(),
            Event::MouseDown(e) => e.handled.get(),
            Event::TouchUpdate(e) => e
                .touches
                .iter()
                .find(|t| t.state == TouchState::Start)
                .map_or(Area::Empty, |t| t.handled.get()),
            _ => Area::Empty,
        }
    }
}

impl Event {
    pub fn hits(&self, cx: &mut Cx, area: Area) -> Hit {
        self.hits_with_options(cx, area, HitOptions::default())
    }

    pub fn hits_with_test<F>(&self, cx: &mut Cx, area: Area, hit_test: F) -> Hit
    where
        F: Fn(Vec2d, &Rect, &Option<Inset>) -> bool,
    {
        self.hits_with_options_and_test(cx, area, HitOptions::new(), hit_test)
    }

    pub fn hits_with_sweep_area(&self, cx: &mut Cx, area: Area, sweep_area: Area) -> Hit {
        self.hits_with_options(cx, area, HitOptions::new().with_sweep_area(sweep_area))
    }

    pub fn hits_with_capture_overload(
        &self,
        cx: &mut Cx,
        area: Area,
        capture_overload: bool,
    ) -> Hit {
        self.hits_with_options(
            cx,
            area,
            HitOptions::new().with_capture_overload(capture_overload),
        )
    }

    pub fn hits_with_options(&self, cx: &mut Cx, area: Area, options: HitOptions) -> Hit {
        self.hits_with_options_and_test(cx, area, options, |abs, rect, margin| {
            Inset::rect_contains_with_inset(abs, rect, margin)
        })
    }

    pub fn hits_with_options_and_test<F>(
        &self,
        cx: &mut Cx,
        area: Area,
        options: HitOptions,
        hit_test: F,
    ) -> Hit
    where
        F: Fn(Vec2d, &Rect, &Option<Inset>) -> bool,
    {
        // Draw lists attach when they end, so validate only between draws.
        if !matches!(self, Event::Draw(_)) {
            cx.validate_scoped_sweep_locks();
        }
        // A cancelled press ends before anything else — even for an area
        // that is no longer drawn: its terminal FingerUp is owed to it
        // whatever became of its drawable.
        if let Some(hit) = self.cancelled_press_hit(cx, area) {
            return hit;
        }
        if !area.is_valid(cx) {
            return Hit::Nothing;
        }
        match self {
            Event::KeyFocus(kf) => {
                if area == kf.prev {
                    return Hit::KeyFocusLost(kf.clone());
                } else if area == kf.focus {
                    return Hit::KeyFocus(kf.clone());
                }
            }
            Event::KeyDown(kd) => {
                if cx.keyboard.has_key_focus(area) {
                    return Hit::KeyDown(kd.clone());
                }
            }
            Event::KeyUp(ku) => {
                if cx.keyboard.has_key_focus(area) {
                    return Hit::KeyUp(ku.clone());
                }
            }
            Event::TextInput(ti) => {
                if cx.keyboard.has_key_focus(area) {
                    return Hit::TextInput(ti.clone());
                }
            }
            Event::TextRangeReplace(tr) => {
                if cx.keyboard.has_key_focus(area) {
                    return Hit::TextRangeReplace(tr.clone());
                }
            }
            // A restricted isolate's widgets can't export text to the clipboard.
            Event::TextCopy(tc) => {
                if cx.keyboard.has_key_focus(area) && !cx.script_data.std.host_io_only() {
                    return Hit::TextCopy(tc.clone());
                }
            }
            Event::TextCut(tc) => {
                if cx.keyboard.has_key_focus(area) && !cx.script_data.std.host_io_only() {
                    return Hit::TextCut(tc.clone());
                }
            }
            Event::ImeAction(ia) => {
                if cx.keyboard.has_key_focus(area) {
                    return Hit::ImeAction(ia.clone());
                }
            }
            Event::SelectionHandleDrag(e) => {
                if cx.keyboard.has_key_focus(area) {
                    return Hit::SelectionHandleDrag(e.clone());
                }
            }
            Event::Scroll(e) => {
                if cx.fingers.test_sweep_lock(options.sweep_area) {
                    return Hit::Nothing;
                }
                if !cx.is_scrolling_allowed_within(&area) {
                    return Hit::Nothing;
                }
                let digit_id = live_id!(mouse).into();

                let rect = area.clipped_rect(&cx);
                if hit_test(e.abs, &rect, &options.margin) {
                    let device = DigitDevice::Mouse {
                        button: MouseButton::PRIMARY,
                    };
                    return Hit::FingerScroll(FingerScrollEvent {
                        abs: e.abs,
                        rect,
                        window_id: e.window_id,
                        digit_id,
                        device,
                        modifiers: e.modifiers,
                        time: e.time,
                        scroll: e.scroll,
                        phase: e.phase,
                    });
                }
            }
            Event::Pinch(e) => {
                if cx.fingers.test_sweep_lock(options.sweep_area) {
                    return Hit::Nothing;
                }
                let rect = area.clipped_rect(&cx);
                if hit_test(e.abs, &rect, &options.margin) {
                    return Hit::FingerPinch(FingerPinchEvent {
                        window_id: e.window_id,
                        abs: e.abs,
                        scale: e.scale,
                        phase: e.phase,
                        modifiers: e.modifiers,
                        time: e.time,
                        rect,
                    });
                }
            }
            Event::TouchUpdate(e) => {
                if cx.fingers.test_sweep_lock(options.sweep_area) {
                    return Hit::Nothing;
                }
                for t in &e.touches {
                    let digit_id = live_id_num!(touch, t.uid).into();
                    let device = DigitDevice::Touch { uid: t.uid };
                    // A finger this area's press was taken from: it already
                    // had its terminal hit; nothing more of it here.
                    if cx.fingers.cancelled.iter().any(|c| c.digit_id == digit_id && c.area == area) {
                        continue;
                    }

                    match t.state {
                        TouchState::Start => {
                            // someone did a second call on our area
                            if cx.fingers.find_digit_for_captured_area(area).is_some() {
                                let rect = area.clipped_rect(&cx);
                                // A second touch landing on an area that already captured one
                                // belongs to that area (e.g. the second finger of a pinch), so
                                // mark it handled to keep widgets behind us from capturing it.
                                // The emptiness check preserves the claim of a widget (e.g. a
                                // child) that already handled this touch earlier in dispatch.
                                if t.handled.get().is_empty()
                                    && hit_test(t.abs, &rect, &options.margin_for(&device))
                                {
                                    t.handled.set(area);
                                }
                                return Hit::FingerDown(FingerDownEvent {
                                    window_id: e.window_id,
                                    abs: t.abs,
                                    digit_id,
                                    device,
                                    tap_count: cx.fingers.tap_count(),
                                    modifiers: e.modifiers,
                                    time: e.time,
                                    rect,
                                });
                            }

                            if !options.capture_overload && !t.handled.get().is_empty() {
                                continue;
                            }

                            if cx.fingers.find_area_capture(area).is_some() {
                                continue;
                            }

                            let rect = area.clipped_rect(&cx);
                            // Hit-test against the touch centroid only — do NOT inflate the
                            // widget rect by `t.radius`. UITouch.majorRadius (and the Android
                            // equivalent) is the contact area's radius, not a "give me extra
                            // hit padding" instruction, and inflating by it leads to surprising
                            // captures: the iOS Simulator reports a `majorRadius` of ~25-40pt
                            // for mouse-as-touch, which makes every button capture clicks ~30pt
                            // outside its visible bounds. UIKit/AppKit hit-test on the centroid;
                            // we match that. Apps that genuinely need a larger hit zone should
                            // pass it explicitly via `HitOptions::margin`.
                            if !hit_test(t.abs, &rect, &options.margin_for(&device)) {
                                continue;
                            }

                            cx.fingers.capture_digit(
                                digit_id,
                                area,
                                options.sweep_area,
                                e.time,
                                t.abs,
                            );

                            t.handled.set(area);
                            return Hit::FingerDown(FingerDownEvent {
                                window_id: e.window_id,
                                abs: t.abs,
                                digit_id,
                                device,
                                tap_count: cx.fingers.tap_count(),
                                modifiers: e.modifiers,
                                time: e.time,
                                rect,
                            });
                        }
                        TouchState::Stop => {
                            let tap_count = cx.fingers.tap_count();
                            let rect = area.clipped_rect(&cx);
                            if let Some(capture) = cx.fingers.find_digit_area_capture(digit_id, area) {
                                // See the note in TouchState::Start above: hit-test on the
                                // touch centroid only, without inflating by `t.radius`.
                                let rect_check = rect.contains(t.abs);

                                // Layout shift fallback: also treat as "over" if finger didn't move
                                // significantly from start (handles keyboard dismissal moving widgets)
                                let layout_shift_fallback = (e.time - capture.time
                                    < TAP_COUNT_TIME)
                                    && ((t.abs - capture.abs_start).length() < TAP_COUNT_DISTANCE);

                                let is_over = !capture.cancelled && (rect_check || layout_shift_fallback);

                                return Hit::FingerUp(FingerUpEvent {
                                    abs_start: capture.abs_start,
                                    rect,
                                    window_id: e.window_id,
                                    abs: t.abs,
                                    digit_id,
                                    device,
                                    has_long_press_occurred: capture.has_long_press_occurred,
                                    tap_count,
                                    capture_time: capture.time,
                                    modifiers: e.modifiers,
                                    time: e.time,
                                    is_over,
                                    is_sweep: false,
                                    cancelled: capture.cancelled,
                                });
                            }
                        }
                        TouchState::Move => {
                            let tap_count = cx.fingers.tap_count();
                            //let hover_last = cx.fingers.get_hover_area(digit_id);
                            let rect = area.clipped_rect(&cx);

                            //let handled_area = t.handled.get();
                            if !options.sweep_area.is_empty() {
                                if let Some(capture) = cx.fingers.find_digit_capture(digit_id) {
                                    if capture.switch_capture.is_none()
                                        && hit_test(t.abs, &rect, &options.margin_for(&device))
                                    {
                                        if t.handled.get().is_empty() {
                                            t.handled.set(area);
                                            if capture.area == area {
                                                return Hit::FingerMove(FingerMoveEvent {
                                                    window_id: e.window_id,
                                                    abs: t.abs,
                                                    digit_id,
                                                    device,
                                                    has_long_press_occurred: capture
                                                        .has_long_press_occurred,
                                                    tap_count,
                                                    modifiers: e.modifiers,
                                                    time: e.time,
                                                    abs_start: capture.abs_start,
                                                    rect,
                                                    is_over: true,
                                                });
                                            } else if capture.sweep_area == options.sweep_area {
                                                // take over the capture
                                                capture.switch_capture = Some(area);
                                                return Hit::FingerDown(FingerDownEvent {
                                                    window_id: e.window_id,
                                                    abs: t.abs,
                                                    digit_id,
                                                    device,
                                                    tap_count: cx.fingers.tap_count(),
                                                    modifiers: e.modifiers,
                                                    time: e.time,
                                                    rect,
                                                });
                                            }
                                        }
                                    } else if capture.area == area {
                                        // we are not over the area
                                        if capture.switch_capture.is_none() {
                                            capture.switch_capture = Some(Area::Empty);
                                        }
                                        return Hit::FingerUp(FingerUpEvent {
                                            abs_start: capture.abs_start,
                                            rect,
                                            window_id: e.window_id,
                                            abs: t.abs,
                                            digit_id,
                                            device,
                                            has_long_press_occurred: capture
                                                .has_long_press_occurred,
                                            tap_count,
                                            capture_time: capture.time,
                                            modifiers: e.modifiers,
                                            time: e.time,
                                            is_sweep: true,
                                            cancelled: capture.cancelled,
                                            is_over: false,
                                        });
                                    }
                                }
                            } else if let Some(capture) = cx.fingers.find_digit_area_capture(digit_id, area) {
                                let is_over = hit_test(t.abs, &rect, &options.margin_for(&device));
                                return Hit::FingerMove(FingerMoveEvent {
                                    window_id: e.window_id,
                                    abs: t.abs,
                                    digit_id,
                                    device,
                                    has_long_press_occurred: capture.has_long_press_occurred,
                                    tap_count,
                                    modifiers: e.modifiers,
                                    time: e.time,
                                    abs_start: capture.abs_start,
                                    rect,
                                    is_over,
                                });
                            }
                        }
                        TouchState::Stable => {}
                    }
                }
            }
            Event::MouseMove(e) => {
                // ok so we dont get hovers
                if cx.fingers.test_sweep_lock(options.sweep_area) {
                    return Hit::Nothing;
                }

                let digit_id = live_id!(mouse).into();

                let tap_count = cx.fingers.tap_count();
                let hover_last = cx.fingers.find_hover_area(digit_id);
                let rect = area.clipped_rect(&cx);

                if let Some((button, _window_id)) = cx.fingers.first_mouse_button {
                    let device = DigitDevice::Mouse { button };
                    //let handled_area = e.handled.get();
                    if !options.sweep_area.is_empty() {
                        if let Some(capture) = cx.fingers.find_digit_capture(digit_id) {
                            if capture.switch_capture.is_none()
                                && hit_test(e.abs, &rect, &options.margin)
                            {
                                if e.handled.get().is_empty() {
                                    e.handled.set(area);
                                    if capture.area == area {
                                        return Hit::FingerMove(FingerMoveEvent {
                                            window_id: e.window_id,
                                            abs: e.abs,
                                            digit_id,
                                            device,
                                            has_long_press_occurred: capture
                                                .has_long_press_occurred,
                                            tap_count,
                                            modifiers: e.modifiers,
                                            time: e.time,
                                            abs_start: capture.abs_start,
                                            rect,
                                            is_over: true,
                                        });
                                    } else if capture.sweep_area == options.sweep_area {
                                        // take over the capture
                                        capture.switch_capture = Some(area);
                                        cx.fingers.new_hover_area(digit_id, area);
                                        return Hit::FingerDown(FingerDownEvent {
                                            window_id: e.window_id,
                                            abs: e.abs,
                                            digit_id,
                                            device,
                                            tap_count: cx.fingers.tap_count(),
                                            modifiers: e.modifiers,
                                            time: e.time,
                                            rect,
                                        });
                                    }
                                }
                            } else if capture.area == area {
                                // we are not over the area
                                if capture.switch_capture.is_none() {
                                    capture.switch_capture = Some(Area::Empty);
                                }
                                return Hit::FingerUp(FingerUpEvent {
                                    abs_start: capture.abs_start,
                                    rect,
                                    window_id: e.window_id,
                                    abs: e.abs,
                                    digit_id,
                                    device,
                                    has_long_press_occurred: capture.has_long_press_occurred,
                                    tap_count,
                                    capture_time: capture.time,
                                    modifiers: e.modifiers,
                                    time: e.time,
                                    is_sweep: true,
                                    cancelled: capture.cancelled,
                                    is_over: false,
                                });
                            }
                        }
                    } else if let Some(capture) = cx.fingers.find_area_capture(area) {
                        let event = Hit::FingerMove(FingerMoveEvent {
                            window_id: e.window_id,
                            abs: e.abs,
                            digit_id,
                            device,
                            has_long_press_occurred: capture.has_long_press_occurred,
                            tap_count,
                            modifiers: e.modifiers,
                            time: e.time,
                            abs_start: capture.abs_start,
                            rect,
                            is_over: hit_test(e.abs, &rect, &options.margin),
                        });
                        cx.fingers.new_hover_area(digit_id, area);
                        return event;
                    }
                } else {
                    // Absolute pin semantics: while a capture carries a
                    // scrub pin the pointer exists only for its owner — no
                    // hover resolution anywhere (the virtual abs must never
                    // highlight widgets it wanders over).
                    if cx.fingers.has_pinned_capture() {
                        return Hit::Nothing;
                    }
                    let device = DigitDevice::Mouse {
                        button: MouseButton::PRIMARY,
                    };

                    let handled_area = e.handled.get();

                    let fhe = FingerHoverEvent {
                        window_id: e.window_id,
                        abs: e.abs,
                        digit_id,
                        device,
                        modifiers: e.modifiers,
                        time: e.time,
                        rect,
                    };

                    if hover_last == area {
                        if (handled_area.is_empty() || handled_area == area)
                            && hit_test(e.abs, &rect, &options.margin)
                        {
                            e.handled.set(area);
                            cx.fingers.new_hover_area(digit_id, area);
                            return Hit::FingerHoverOver(fhe);
                        } else {
                            return Hit::FingerHoverOut(fhe);
                        }
                    } else {
                        if (handled_area.is_empty() || handled_area == area)
                            && hit_test(e.abs, &rect, &options.margin)
                        {
                            //let any_captured = cx.fingers.get_digit_for_captured_area(area);
                            cx.fingers.new_hover_area(digit_id, area);
                            e.handled.set(area);
                            return Hit::FingerHoverIn(fhe);
                        }
                    }
                }
            }
            Event::MouseDown(e) => {
                // Absolute pin semantics: no NEW captures while a capture
                // carries a scrub pin — a press at the virtual abs must not
                // engage whatever widget it happens to sit over. (The
                // owner's own press predates the pin; cancel gestures
                // listen to raw events, not hits.)
                if cx.fingers.has_pinned_capture() {
                    return Hit::Nothing;
                }
                let digit_id = live_id!(mouse).into();

                let device = DigitDevice::Mouse { button: e.button };

                // if we already captured it just return it immediately
                if cx.fingers.find_digit_for_captured_area(area).is_some() {
                    let rect = area.clipped_rect(&cx);
                    return Hit::FingerDown(FingerDownEvent {
                        window_id: e.window_id,
                        abs: e.abs,
                        digit_id,
                        device,
                        tap_count: cx.fingers.tap_count(),
                        modifiers: e.modifiers,
                        time: e.time,
                        rect,
                    });
                }

                if cx.fingers.test_sweep_lock(options.sweep_area) {
                    return Hit::Nothing;
                }

                if !options.capture_overload && !e.handled.get().is_empty() {
                    return Hit::Nothing;
                }

                if cx.fingers.first_mouse_button.is_some()
                    && cx.fingers.first_mouse_button.unwrap().0 != e.button
                {
                    return Hit::Nothing;
                }

                let rect = area.clipped_rect(&cx);
                if !hit_test(e.abs, &rect, &options.margin) {
                    return Hit::Nothing;
                }

                if cx.fingers.find_digit_for_captured_area(area).is_some() {
                    return Hit::Nothing;
                }

                cx.fingers
                    .capture_digit(digit_id, area, options.sweep_area, e.time, e.abs);
                e.handled.set(area);
                cx.fingers.new_hover_area(digit_id, area);
                return Hit::FingerDown(FingerDownEvent {
                    window_id: e.window_id,
                    abs: e.abs,
                    digit_id,
                    device,
                    tap_count: cx.fingers.tap_count(),
                    modifiers: e.modifiers,
                    time: e.time,
                    rect,
                });
            }
            Event::MouseUp(e) => {
                if cx.fingers.test_sweep_lock(options.sweep_area) {
                    return Hit::Nothing;
                }

                if cx.fingers.first_mouse_button.is_some()
                    && cx.fingers.first_mouse_button.unwrap().0 != e.button
                {
                    return Hit::Nothing;
                }

                let digit_id = live_id!(mouse).into();

                let device = DigitDevice::Mouse { button: e.button };
                let tap_count = cx.fingers.tap_count();
                let rect = area.clipped_rect(&cx);

                if let Some(capture) = cx.fingers.find_area_capture(area) {
                    let is_over = !capture.cancelled && hit_test(e.abs, &rect, &options.margin);
                    let event = Hit::FingerUp(FingerUpEvent {
                        abs_start: capture.abs_start,
                        rect,
                        window_id: e.window_id,
                        abs: e.abs,
                        digit_id,
                        device,
                        has_long_press_occurred: capture.has_long_press_occurred,
                        tap_count,
                        capture_time: capture.time,
                        modifiers: e.modifiers,
                        time: e.time,
                        is_over,
                        is_sweep: false,
                        cancelled: capture.cancelled,
                    });
                    if is_over {
                        cx.fingers.new_hover_area(digit_id, area);
                    }
                    return event;
                }
            }
            Event::MouseLeave(e) => {
                if cx.fingers.test_sweep_lock(options.sweep_area) {
                    return Hit::Nothing;
                }
                let device = DigitDevice::Mouse {
                    button: MouseButton::empty(),
                };
                let digit_id = live_id!(mouse).into();
                let rect = area.clipped_rect(&cx);
                let hover_last = cx.fingers.find_hover_area(digit_id);
                let handled_area = e.handled.get();

                let fhe = FingerHoverEvent {
                    window_id: e.window_id,
                    abs: e.abs,
                    digit_id,
                    device,
                    modifiers: e.modifiers,
                    time: e.time,
                    rect,
                };
                if hover_last == area {
                    return Hit::FingerHoverOut(fhe);
                }
            }
            Event::LongPress(e) => {
                if cx.fingers.test_sweep_lock(options.sweep_area) {
                    return Hit::Nothing;
                }

                let rect = area.clipped_rect(&cx);
                if let Some(capture) = cx.fingers.find_area_capture(area).filter(|c| !c.cancelled) {
                    capture.has_long_press_occurred = true;
                    // No hit test is needed because we already did that in the previous
                    // FingerDown `capture` event that started the long press.
                    // Also, there is no need to include the starting position (`abs_start`)
                    // since it will always be identical to the `abs` position of the original capture.
                    let digit_id = live_id_num!(touch, e.uid).into();
                    let device = DigitDevice::Touch { uid: e.uid };
                    return Hit::FingerLongPress(FingerLongPressEvent {
                        window_id: e.window_id,
                        abs: e.abs,
                        capture_time: capture.time,
                        time: e.time,
                        digit_id,
                        device,
                        rect,
                    });
                }
            }
            Event::XrLocal(e) => return e.hits_with_options_and_test(cx, area, options, hit_test),
            _ => (),
        };
        Hit::Nothing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_claim_ends_when_its_digit_is_released() {
        let mut fingers = CxFingers::default();
        fingers.claim_drag(live_id_num!(touch, 3).into());
        assert!(fingers.is_touch_drag_claimed(3));
        assert!(!fingers.is_touch_drag_claimed(4));
        // Touch uids get reused, so a claim that outlived its touch would block the next one.
        fingers.release_digit(live_id_num!(touch, 3).into());
        assert!(!fingers.is_touch_drag_claimed(3));
    }

    use crate::{
        area::RectArea,
        draw_list::{CxRectArea, DrawList},
    };

    const INNER: usize = 0;
    const OUTER: usize = 1;
    /// Over the inner scrollable, which covers the top of the outer one.
    const OVER_INNER: Vec2d = dvec2(50.0, 50.0);
    const OVER_OUTER: Vec2d = dvec2(50.0, 300.0);

    /// What one scroll event did: whether its y axis was handled before dispatch began,
    /// and which scrollable took each axis.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Took {
        was_handled: bool,
        x: Option<usize>,
        y: Option<usize>,
    }

    /// A scrollable nested in another. Scroll events reach them innermost first, the way
    /// widgets dispatch them, with the outer one passing them on as a scroll container.
    struct Nest {
        cx: Cx,
        draw_lists: [DrawList; 2],
        areas: Rc<Cell<[Area; 2]>>,
        /// Per scrollable, whether it can scroll along y.
        are_scrollable: Rc<Cell<[bool; 2]>>,
        /// Per scrollable, whether it can scroll along x.
        are_scrollable_x: Rc<Cell<[bool; 2]>>,
        /// Whether the inner one, like a list, takes scrolls it can't move on when nothing around it can.
        is_inner_list: Rc<Cell<bool>>,
        is_drawn: Rc<Cell<[bool; 2]>>,
        took: Rc<RefCell<Vec<Took>>>,
        time: f64,
        /// How far each scroll event moves.
        delta: Vec2d,
    }

    impl Nest {
        fn new() -> Self {
            Self::with_inner_rect(Rect { pos: dvec2(0.0, 0.0), size: dvec2(200.0, 100.0) })
        }

        /// The outer scrollable is 200 x 400 at the origin, and everything is clipped to its rect.
        fn with_inner_rect(inner_rect: Rect) -> Self {
            let areas = Rc::new(Cell::new([Area::Empty; 2]));
            let are_scrollable = Rc::new(Cell::new([true; 2]));
            let are_scrollable_x = Rc::new(Cell::new([false; 2]));
            let is_inner_list = Rc::new(Cell::new(false));
            let is_drawn = Rc::new(Cell::new([true; 2]));
            let took = Rc::new(RefCell::new(Vec::new()));
            let (shared_areas, scrollable_y, scrollable_x, is_list, drawn, log) = (
                areas.clone(),
                are_scrollable.clone(),
                are_scrollable_x.clone(),
                is_inner_list.clone(),
                is_drawn.clone(),
                took.clone(),
            );
            let mut cx = Cx::new(Box::new(move |cx, event| {
                let Event::Scroll(e) = event else { return };
                let was_handled = e.handled_y.get();
                let areas = shared_areas.get();
                let is_over = |cx: &Cx, i: usize| areas[i].clipped_rect(cx).contains(e.abs);
                let scrollable_axes = |i: usize| [scrollable_x.get()[i], scrollable_y.get()[i]];
                let mut took = [None; 2];
                let mut take = |cx: &mut Cx, i: usize| {
                    // A scrollable that isn't drawn gets no events.
                    if !drawn.get()[i] {
                        return;
                    }
                    for (slot, axis) in [Vec2Index::X, Vec2Index::Y].into_iter().enumerate() {
                        // A list only scrolls along y here.
                        let is_fallback = i == INNER && axis == Vec2Index::Y && is_list.get()
                            && !cx.is_inside_scroll_container(axis);
                        let is_candidate = is_over(cx, i) && (scrollable_axes(i)[slot] || is_fallback);
                        if cx.is_scroll_owner(e, areas[i], axis, is_candidate) {
                            assert!(took[slot].is_none(), "two scrollables took one scroll event");
                            took[slot] = Some(i);
                        }
                    }
                };
                let outer_axes = scrollable_axes(OUTER).map(|is_scrollable| is_scrollable && is_over(cx, OUTER));
                cx.begin_scroll_container(outer_axes);
                take(cx, INNER);
                cx.end_scroll_container(outer_axes);
                take(cx, OUTER);
                log.borrow_mut().push(Took { was_handled, x: took[0], y: took[1] });
            }));
            let draw_lists = [DrawList::new(&mut cx), DrawList::new(&mut cx)];
            let outer_rect = Rect { pos: dvec2(0.0, 0.0), size: dvec2(200.0, 400.0) };
            let mut new_areas = [Area::Empty; 2];
            for (i, rect) in [inner_rect, outer_rect].into_iter().enumerate() {
                let list = &mut cx.draw_lists[draw_lists[i].id()];
                list.rect_areas.push(CxRectArea {
                    rect,
                    draw_clip: (outer_rect.pos, outer_rect.pos + outer_rect.size),
                });
                new_areas[i] = Area::Rect(RectArea {
                    draw_list_id: draw_lists[i].id(),
                    rect_id: 0,
                    redraw_id: list.redraw_id,
                });
            }
            areas.set(new_areas);
            Self { cx, draw_lists, areas, are_scrollable, are_scrollable_x, is_inner_list, is_drawn, took, time: 1.0, delta: dvec2(10.0, 10.0) }
        }

        fn area(&self, i: usize) -> Area {
            self.areas.get()[i]
        }

        /// Takes the inner scrollable out of the UI, leaving its area stale.
        fn remove_inner(&mut self) {
            self.cx.draw_lists[self.draw_lists[INNER].id()].redraw_id += 1;
            self.is_drawn.set([false, true]);
        }

        /// Draws another scrollable where the inner one was, the way a list reuses the spot of a row it dropped.
        fn replace_inner(&mut self) {
            self.remove_inner();
            let mut areas = self.areas.get();
            if let Area::Rect(area) = &mut areas[INNER] {
                area.redraw_id = self.cx.draw_lists[self.draw_lists[INNER].id()].redraw_id;
            }
            self.areas.set(areas);
            self.is_drawn.set([true, true]);
        }

        fn event(&mut self, abs: Vec2d, phase: ScrollPhase, dt: f64) -> ScrollEvent {
            self.time += dt;
            ScrollEvent {
                window_id: WindowId(0, 0),
                scroll: self.delta,
                abs,
                modifiers: KeyModifiers::default(),
                handled_x: Cell::new(false),
                handled_y: Cell::new(false),
                is_mouse: phase == ScrollPhase::None,
                time: self.time,
                phase,
            }
        }

        /// Dispatches one scroll event 16 ms after the last and returns which scrollable took its y axis.
        fn scroll(&mut self, abs: Vec2d, phase: ScrollPhase) -> Option<usize> {
            self.scroll_after(abs, phase, 0.016)
        }

        fn scroll_after(&mut self, abs: Vec2d, phase: ScrollPhase, dt: f64) -> Option<usize> {
            self.dispatch(abs, phase, dt).y
        }

        /// Like [`Self::scroll`], returning which scrollable took each axis (x, y).
        fn scroll_xy(&mut self, abs: Vec2d, phase: ScrollPhase) -> (Option<usize>, Option<usize>) {
            let took = self.dispatch(abs, phase, 0.016);
            (took.x, took.y)
        }

        /// Like [`Self::scroll_xy`] over the inner scrollable, moving `delta`.
        fn scroll_by(&mut self, phase: ScrollPhase, delta: Vec2d) -> (Option<usize>, Option<usize>) {
            self.delta = delta;
            self.scroll_xy(OVER_INNER, phase)
        }

        fn dispatch(&mut self, abs: Vec2d, phase: ScrollPhase, dt: f64) -> Took {
            let e = self.event(abs, phase, dt);
            self.cx.call_event_handler(&Event::Scroll(e));
            *self.took.borrow().last().unwrap()
        }

        /// Moves the pointer to `abs`, `dt` seconds after the last event.
        fn move_pointer(&mut self, abs: Vec2d, dt: f64) {
            self.time += dt;
            self.cx.call_event_handler(&Event::MouseMove(MouseMoveEvent {
                abs,
                lock_delta: dvec2(0.0, 0.0),
                window_id: WindowId(0, 0),
                modifiers: KeyModifiers::default(),
                time: self.time,
                handled: Cell::new(Area::Empty),
            }));
        }

        fn press(&mut self) {
            self.time += 0.016;
            self.cx.call_event_handler(&Event::MouseDown(MouseDownEvent {
                abs: OVER_INNER,
                button: MouseButton::PRIMARY,
                window_id: WindowId(0, 0),
                modifiers: KeyModifiers::default(),
                handled: Cell::new(Area::Empty),
                time: self.time,
            }));
        }
    }

    #[test]
    fn a_trackpad_gesture_stays_with_the_scrollable_it_began_on() {
        let mut nest = Nest::new();
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Began), Some(INNER));
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Changed), Some(INNER));
        // Neither the pointer leaving it nor running out of content hands the gesture on.
        nest.are_scrollable.set([false, true]);
        assert_eq!(nest.scroll(OVER_OUTER, ScrollPhase::Changed), Some(INNER));
        assert_eq!(nest.scroll(OVER_OUTER, ScrollPhase::Ended), Some(INNER));
        assert_eq!(nest.scroll(OVER_OUTER, ScrollPhase::Momentum), Some(INNER));
        assert_eq!(nest.scroll(OVER_OUTER, ScrollPhase::MomentumEnded), Some(INNER));
        // The next gesture is decided afresh.
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Began), Some(OUTER));
        nest.are_scrollable.set([true, true]);
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Changed), Some(OUTER));
    }

    #[test]
    fn a_gesture_without_a_began_phase_is_latched_from_its_first_event() {
        // Wayland touchpads start with `Changed`.
        let mut nest = Nest::new();
        assert_eq!(nest.scroll(OVER_OUTER, ScrollPhase::Changed), Some(OUTER));
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Changed), Some(OUTER));
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Ended), Some(OUTER));
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Changed), Some(INNER));
    }

    #[test]
    fn a_nested_scrollable_with_nothing_to_scroll_leaves_the_scroll_to_its_ancestor() {
        let mut nest = Nest::new();
        nest.are_scrollable.set([false, true]);
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::None), Some(OUTER));
        assert_eq!(nest.scroll_after(OVER_INNER, ScrollPhase::Began, 1.0), Some(OUTER));
        nest.are_scrollable.set([false, false]);
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Began), None);
    }

    #[test]
    fn a_list_with_nothing_to_scroll_takes_the_scroll_when_nothing_around_it_can() {
        // So it still rubber-bands, and counts the travel.
        let mut nest = Nest::new();
        nest.is_inner_list.set(true);
        nest.are_scrollable.set([false, true]);
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::None), Some(OUTER));
        nest.are_scrollable.set([false, false]);
        assert_eq!(nest.scroll_after(OVER_INNER, ScrollPhase::None, 1.0), Some(INNER));
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Began), Some(INNER));
        assert_eq!(nest.scroll(OVER_OUTER, ScrollPhase::Began), None);
        // A container only counts while it passes the event to its children.
        assert!(!nest.cx.is_inside_scroll_container(Vec2Index::Y));
    }

    #[test]
    fn wheel_scrolls_stay_together_until_they_pause() {
        let mut nest = Nest::new();
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::None), Some(INNER));
        nest.are_scrollable.set([false, true]);
        assert_eq!(nest.scroll_after(OVER_INNER, ScrollPhase::None, 0.4), Some(INNER));
        // The hand drifting, even off the owner, doesn't hand the scroll on.
        assert_eq!(nest.scroll(OVER_INNER + dvec2(0.0, 30.0), ScrollPhase::None), Some(INNER));
        assert_eq!(nest.scroll(OVER_OUTER, ScrollPhase::None), Some(INNER));
        // A pause longer than the timeout starts a new scroll.
        let after_pause = WHEEL_LATCH_TIMEOUT + 0.1;
        assert_eq!(nest.scroll_after(OVER_INNER, ScrollPhase::None, after_pause), Some(OUTER));
    }

    #[test]
    fn a_wheel_scroll_nothing_took_goes_to_whatever_can_take_it() {
        // Nothing under the pointer can scroll, so nothing is latched.
        let mut nest = Nest::new();
        nest.are_scrollable.set([true, false]);
        assert_eq!(nest.scroll(OVER_OUTER, ScrollPhase::None), None);
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::None), Some(INNER));
        assert_eq!(nest.scroll(OVER_OUTER, ScrollPhase::None), Some(INNER));
    }

    #[test]
    fn moving_the_pointer_off_the_owner_once_the_wheel_rests_ends_its_scroll() {
        let mut nest = Nest::new();
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::None), Some(INNER));
        // Moves while the wheel turns, or within the owner, keep it.
        nest.move_pointer(OVER_OUTER, 0.05);
        assert_eq!(nest.scroll(OVER_OUTER, ScrollPhase::None), Some(INNER));
        nest.move_pointer(OVER_INNER + dvec2(0.0, 30.0), WHEEL_LATCH_MOVE_DELAY + 0.05);
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::None), Some(INNER));
        nest.move_pointer(OVER_OUTER, WHEEL_LATCH_MOVE_DELAY + 0.05);
        assert_eq!(nest.scroll(OVER_OUTER, ScrollPhase::None), Some(OUTER));
        // A trackpad gesture follows its fingers, not the pointer.
        let mut nest = Nest::new();
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Began), Some(INNER));
        nest.move_pointer(OVER_OUTER, WHEEL_LATCH_MOVE_DELAY + 0.05);
        assert_eq!(nest.scroll(OVER_OUTER, ScrollPhase::Changed), Some(INNER));
    }

    #[test]
    fn an_axis_nobody_took_when_the_gesture_began_stays_unowned() {
        let mut nest = Nest::new();
        nest.are_scrollable.set([false, false]);
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Began), None);
        nest.are_scrollable.set([true, true]);
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Changed), None);
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Ended), None);
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Momentum), None);
    }

    #[test]
    fn momentum_goes_only_to_the_trackpad_gesture_it_follows() {
        let mut nest = Nest::new();
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Momentum), None);
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::None), Some(INNER));
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Momentum), None);
        // An old stream still ending after the next gesture began reaches nothing.
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Began), Some(INNER));
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Momentum), None);
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::MomentumEnded), None);
        nest.are_scrollable.set([false, true]);
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Changed), Some(INNER));
    }

    #[test]
    fn a_press_ends_the_scroll_in_progress() {
        let mut nest = Nest::new();
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::None), Some(INNER));
        nest.are_scrollable.set([false, true]);
        nest.press();
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::None), Some(OUTER));
        // Momentum after a press (which caught the coast) moves nothing.
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Began), Some(OUTER));
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Ended), Some(OUTER));
        nest.press();
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Momentum), None);
    }

    #[test]
    fn a_touch_on_the_trackpad_reaches_the_owner_without_taking_anything() {
        let mut nest = Nest::new();
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Began), Some(INNER));
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Ended), Some(INNER));
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Touched), Some(INNER));
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::MomentumEnded), Some(INNER));
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Touched), None);
    }

    #[test]
    fn owned_axes_are_marked_handled_before_dispatch() {
        let mut nest = Nest::new();
        nest.scroll(OVER_INNER, ScrollPhase::Began);
        nest.scroll(OVER_INNER, ScrollPhase::Changed);
        nest.scroll(OVER_INNER, ScrollPhase::Touched);
        let took: Vec<_> = nest.took.borrow().iter().map(|t| (t.was_handled, t.y)).collect();
        assert_eq!(took[0], (false, Some(INNER)), "a new gesture is free to take");
        assert_eq!(took[1], (true, Some(INNER)), "widgets that only check the flag leave it alone");
        assert_eq!(took[2], (false, Some(INNER)), "every scrollable may stop on a touch");
    }

    #[test]
    fn a_scroll_dispatched_by_hand_goes_to_the_first_scrollable_that_can_take_it() {
        let mut nest = Nest::new();
        let (inner, outer) = (nest.area(INNER), nest.area(OUTER));
        let e = nest.event(OVER_INNER, ScrollPhase::Changed, 0.016);
        assert!(nest.cx.is_scroll_owner(&e, inner, Vec2Index::Y, true));
        assert!(!nest.cx.is_scroll_owner(&e, outer, Vec2Index::Y, true));
        assert!(nest.cx.is_scroll_owner(&e, outer, Vec2Index::X, true));
        let touched = nest.event(OVER_INNER, ScrollPhase::Touched, 0.016);
        assert!(!nest.cx.is_scroll_owner(&touched, inner, Vec2Index::Y, true));
    }

    #[test]
    fn a_removed_owner_gives_up_the_gesture_but_not_its_momentum() {
        let mut nest = Nest::new();
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Began), Some(INNER));
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Ended), Some(INNER));
        nest.remove_inner();
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Momentum), None);
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::MomentumEnded), None);
        // Mid-gesture, the fingers move whatever is under them now.
        let mut nest = Nest::new();
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Began), Some(INNER));
        nest.remove_inner();
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Changed), Some(OUTER));
    }

    #[test]
    fn a_removed_owner_frees_only_its_own_axis() {
        // A strip that scrolls along x, in a list that scrolls along y.
        for is_wheel in [false, true] {
            let phase = if is_wheel { ScrollPhase::None } else { ScrollPhase::Changed };
            let mut nest = Nest::new();
            nest.are_scrollable_x.set([true, false]);
            nest.are_scrollable.set([false, true]);
            assert_eq!(nest.scroll_xy(OVER_INNER, phase), (Some(INNER), Some(OUTER)));
            // The list drops the strip's row, and a scrollable along both axes takes its spot.
            nest.replace_inner();
            nest.are_scrollable_x.set([true, false]);
            nest.are_scrollable.set([true, true]);
            assert_eq!(nest.scroll_xy(OVER_INNER, phase), (Some(INNER), Some(OUTER)));
            assert_eq!(nest.scroll_xy(OVER_INNER, phase), (Some(INNER), Some(OUTER)));
        }
    }

    #[test]
    fn an_open_popup_keeps_scrolls_from_everything_else() {
        // The outer area stands in for a popup holding the sweep lock.
        let mut nest = Nest::new();
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Began), Some(INNER));
        nest.cx.fingers.sweep_lock(nest.area(OUTER));
        // The owner it covers lets go, mid-gesture too.
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::Changed), Some(OUTER));
        assert_eq!(nest.scroll_after(OVER_INNER, ScrollPhase::None, 1.0), Some(OUTER));
        nest.cx.fingers.sweep_unlock(nest.area(OUTER));
        assert_eq!(nest.scroll_after(OVER_INNER, ScrollPhase::None, 1.0), Some(INNER));
    }

    #[test]
    fn scrolling_blocked_outside_an_area_still_reaches_a_partly_hidden_scrollable_inside_it() {
        // The inner scrollable's top is scrolled out of the outer one, like a list in a modal.
        let inner_rect = Rect { pos: dvec2(0.0, -50.0), size: dvec2(200.0, 150.0) };
        let mut nest = Nest::with_inner_rect(inner_rect);
        nest.cx.block_scrolling_except_within(nest.area(OUTER));
        assert_eq!(nest.scroll(OVER_INNER, ScrollPhase::None), Some(INNER));
        assert_eq!(nest.scroll_after(OVER_OUTER, ScrollPhase::None, 1.0), Some(OUTER));
    }

    #[test]
    fn scrolling_blocked_outside_a_drawn_area_still_reaches_a_scrollable_flush_with_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let draw_list = DrawList::new(&mut cx);
        // A page filling a modal on a 1080px wide phone at a 2.625 dpi factor. The modal's rect is
        // rounded to f32 like a drawn area's, while the page's keeps every digit.
        let width = 1080.0 / 2.625 - 30.0;
        let [modal, page, wider_page] = [
            Rect { pos: dvec2(15.0, 30.0), size: dvec2(width as f32 as f64, 804.0) },
            Rect { pos: dvec2(15.0, 83.5), size: dvec2(width, 750.5) },
            Rect { pos: dvec2(15.0, 83.5), size: dvec2(width + 1.0, 750.5) },
        ]
        .map(|rect| {
            let list = &mut cx.draw_lists[draw_list.id()];
            list.rect_areas.push(CxRectArea { rect, draw_clip: (dvec2(0.0, 0.0), dvec2(1000.0, 1000.0)) });
            Area::Rect(RectArea { draw_list_id: draw_list.id(), rect_id: list.rect_areas.len() - 1, redraw_id: list.redraw_id })
        });
        cx.block_scrolling_except_within(modal);
        assert!(cx.is_scrolling_allowed_within(&page));
        assert!(!cx.is_scrolling_allowed_within(&wider_page));
    }

    #[test]
    fn a_drag_scroll_belongs_to_the_scrollable_that_claimed_it_first() {
        let nest = Nest::new();
        let (inner, outer) = (nest.area(INNER), nest.area(OUTER));
        let mut fingers = CxFingers::default();
        let touch: DigitId = live_id_num!(touch, 1).into();
        assert!(fingers.is_scroll_drag_owner(touch, Vec2Index::X, inner));
        assert!(fingers.is_scroll_drag_owner(touch, Vec2Index::X, inner));
        assert!(!fingers.is_scroll_drag_owner(touch, Vec2Index::X, outer));
        // The other axis is free for the outer scrollable.
        assert!(fingers.is_scroll_drag_owner(touch, Vec2Index::Y, outer));
        assert_eq!(fingers.scroll_drag_owner(touch, Vec2Index::Y), Some(outer));
        fingers.release_digit(touch);
        assert_eq!(fingers.scroll_drag_owner(touch, Vec2Index::X), None);
        assert!(fingers.is_scroll_drag_owner(touch, Vec2Index::X, outer));
    }

    /// Another area in the inner scrollable's draw list, like a button inside it.
    fn button_in(area: Area) -> Area {
        let Area::Rect(mut rect) = area else { unreachable!() };
        rect.rect_id = 1;
        Area::Rect(rect)
    }

    fn touch_event(state: TouchState, abs: Vec2d) -> Event {
        Event::TouchUpdate(TouchUpdateEvent {
            time: 1.0,
            window_id: WindowId(0, 0),
            modifiers: KeyModifiers::default(),
            touches: vec![TouchPoint {
                state,
                abs,
                time: 1.0,
                uid: 1,
                rotation_angle: 0.0,
                force: 0.0,
                radius: dvec2(0.0, 0.0),
                handled: Cell::new(Area::Empty),
                sweep_lock: Cell::new(Area::Empty),
            }],
        })
    }

    #[test]
    fn a_scrollable_can_join_a_press_only_drag_scrollers_hold() {
        let nest = Nest::new();
        let (inner, outer) = (nest.area(INNER), nest.area(OUTER));
        let mut fingers = CxFingers::default();
        let touch: DigitId = live_id_num!(touch, 1).into();
        assert!(!fingers.can_join_scroll_drag(touch, Vec2Index::Y, outer), "nothing holds it");
        fingers.capture_digit(touch, inner, Area::Empty, 1.0, OVER_INNER);
        assert!(!fingers.can_join_scroll_drag(touch, Vec2Index::Y, outer), "a plain widget holds it");
        // A drag scroller with nothing to scroll leaves both axes to the scrollable around it.
        fingers.hold_scroll_drag(touch, inner);
        assert!(fingers.can_join_scroll_drag(touch, Vec2Index::X, outer));
        assert!(fingers.can_join_scroll_drag(touch, Vec2Index::Y, outer));
        assert!(fingers.is_scroll_drag_owner(touch, Vec2Index::X, inner));
        assert!(!fingers.can_join_scroll_drag(touch, Vec2Index::X, outer));
        assert!(fingers.can_join_scroll_drag(touch, Vec2Index::Y, outer));
        // A button holding it too keeps it from the outer one.
        fingers.capture_digit(touch, button_in(inner), Area::Empty, 1.0, OVER_INNER);
        assert!(!fingers.can_join_scroll_drag(touch, Vec2Index::Y, outer));
        fingers.release_digit(touch);
        fingers.capture_digit(touch, inner, Area::Empty, 1.0, OVER_INNER);
        fingers.hold_scroll_drag(touch, inner);
        fingers.claim_drag(touch);
        assert!(!fingers.can_join_scroll_drag(touch, Vec2Index::Y, outer), "a widget claimed the drag");
    }

    #[test]
    fn a_scrollable_can_join_a_press_held_by_one_that_took_the_other_axis_without_holding_it() {
        // Like a list, which takes its axis without holding the press.
        let nest = Nest::new();
        let (inner, outer) = (nest.area(INNER), nest.area(OUTER));
        let mut fingers = CxFingers::default();
        let touch: DigitId = live_id_num!(touch, 1).into();
        fingers.capture_digit(touch, inner, Area::Empty, 1.0, OVER_INNER);
        assert!(fingers.is_scroll_drag_owner(touch, Vec2Index::X, inner));
        assert!(fingers.can_join_scroll_drag(touch, Vec2Index::Y, outer));
        assert!(!fingers.can_join_scroll_drag(touch, Vec2Index::X, outer));
    }

    #[test]
    fn a_scrollable_never_joins_a_press_something_outside_it_held_first() {
        let nest = Nest::new();
        let (inner, outer) = (nest.area(INNER), nest.area(OUTER));
        let mut fingers = CxFingers::default();
        let touch: DigitId = live_id_num!(touch, 1).into();
        // The inner scroller is drawn over the outer one, not inside it, so the outer one sees the press last.
        fingers.capture_digit(touch, inner, Area::Empty, 1.0, OVER_INNER);
        fingers.hold_scroll_drag(touch, inner);
        fingers.mark_press_as_held_outside(touch, outer);
        assert!(!fingers.can_join_scroll_drag(touch, Vec2Index::Y, outer));
        fingers.release_digit(touch);
        fingers.capture_digit(touch, inner, Area::Empty, 1.0, OVER_INNER);
        fingers.hold_scroll_drag(touch, inner);
        assert!(fingers.can_join_scroll_drag(touch, Vec2Index::Y, outer), "the mark ends with its press");
    }

    #[test]
    fn a_scrollable_that_joins_a_press_gets_its_moves_and_release() {
        let mut nest = Nest::new();
        let (inner, outer) = (nest.area(INNER), nest.area(OUTER));
        let cx = &mut nest.cx;
        let touch: DigitId = live_id_num!(touch, 1).into();
        let start = touch_event(TouchState::Start, OVER_INNER);
        assert!(matches!(start.hits(cx, inner), Hit::FingerDown(_)));
        cx.fingers.hold_scroll_drag(touch, inner);
        assert!(cx.fingers.is_scroll_drag_owner(touch, Vec2Index::X, inner));
        assert!(matches!(start.hits(cx, outer), Hit::Nothing), "the inner one holds it");
        assert!(cx.fingers.can_join_scroll_drag(touch, Vec2Index::Y, outer));
        assert!(matches!(start.hits_with_capture_overload(cx, outer, true), Hit::FingerDown(_)));
        // The outer's other bar gets the press without the overload.
        assert!(matches!(start.hits(cx, outer), Hit::FingerDown(_)));
        for state in [TouchState::Move, TouchState::Stop] {
            let event = touch_event(state, OVER_INNER + dvec2(0.0, 20.0));
            for area in [inner, outer] {
                let hit = event.hits(cx, area);
                match state {
                    TouchState::Move => assert!(matches!(hit, Hit::FingerMove(_))),
                    _ => assert!(matches!(hit, Hit::FingerUp(_))),
                }
            }
            if let Event::TouchUpdate(e) = &event {
                cx.fingers.process_touch_update_end(&e.touches);
            }
        }
        assert!(!cx.fingers.any_areas_captured());
        assert_eq!(cx.fingers.scroll_drag_owner(touch, Vec2Index::X), None);
        assert!(!cx.fingers.can_join_scroll_drag(touch, Vec2Index::Y, outer));
    }

    #[test]
    fn a_mouse_drag_that_a_scrollable_joins_moves_the_hover_to_it() {
        let mut nest = Nest::new();
        let (inner, outer) = (nest.area(INNER), nest.area(OUTER));
        let cx = &mut nest.cx;
        let mouse: DigitId = live_id!(mouse).into();
        cx.fingers.mouse_down(MouseButton::PRIMARY, WindowId(0, 0));
        let down = Event::MouseDown(MouseDownEvent {
            abs: OVER_INNER,
            button: MouseButton::PRIMARY,
            window_id: WindowId(0, 0),
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 1.0,
        });
        assert!(matches!(down.hits(cx, inner), Hit::FingerDown(_)));
        cx.fingers.hold_scroll_drag(mouse, inner);
        assert!(cx.fingers.is_scroll_drag_owner(mouse, Vec2Index::X, inner));
        assert!(matches!(down.hits(cx, outer), Hit::Nothing));
        assert!(cx.fingers.can_join_scroll_drag(mouse, Vec2Index::Y, outer));
        assert!(matches!(down.hits_with_capture_overload(cx, outer, true), Hit::FingerDown(_)));
        let moved = Event::MouseMove(MouseMoveEvent {
            abs: OVER_INNER + dvec2(0.0, 20.0),
            lock_delta: dvec2(0.0, 0.0),
            window_id: WindowId(0, 0),
            modifiers: KeyModifiers::default(),
            time: 1.1,
            handled: Cell::new(Area::Empty),
        });
        assert!(matches!(moved.hits(cx, inner), Hit::FingerMove(_)));
        assert!(matches!(moved.hits(cx, outer), Hit::FingerMove(_)));
        cx.fingers.cycle_hover_area(mouse);
        assert_eq!(cx.fingers.find_hover_area(mouse), outer, "the last area a move reached");
        let up = Event::MouseUp(MouseUpEvent {
            abs: OVER_INNER + dvec2(0.0, 20.0),
            button: MouseButton::PRIMARY,
            window_id: WindowId(0, 0),
            modifiers: KeyModifiers::default(),
            time: 1.2,
        });
        assert!(matches!(up.hits(cx, inner), Hit::FingerUp(_)));
        assert!(matches!(up.hits(cx, outer), Hit::FingerUp(_)));
        cx.fingers.mouse_up(MouseButton::PRIMARY);
        assert!(!cx.fingers.any_areas_captured());
    }

    #[test]
    fn a_gesture_goes_along_the_axis_whose_compass_point_is_nearest() {
        let direction = |x, y| ScrollDirection::from_travel(dvec2(x, y));
        assert_eq!(direction(4.0, 3.0), None, "not the tap distance yet");
        assert_eq!(direction(10.0, -4.0), Some(ScrollDirection::Along(Vec2Index::X)));
        assert_eq!(direction(-4.0, 10.0), Some(ScrollDirection::Along(Vec2Index::Y)));
        assert_eq!(direction(-10.0, 4.5), Some(ScrollDirection::Free));
        assert_eq!(direction(10.0, 10.0), Some(ScrollDirection::Free));
    }

    #[test]
    fn a_drag_two_scrollers_share_keeps_to_the_way_it_started() {
        let nest = Nest::new();
        let (inner, outer) = (nest.area(INNER), nest.area(OUTER));
        let mut fingers = CxFingers::default();
        let touch: DigitId = live_id_num!(touch, 1).into();
        // Nobody shares it yet, so it moves from the first pixel.
        assert!(fingers.is_scroll_drag_owner(touch, Vec2Index::Y, outer));
        assert_eq!(fingers.lock_scroll_drag(touch, dvec2(1.0, 1.0)), Some(ScrollDirection::Free));
        assert!(fingers.is_scroll_drag_owner(touch, Vec2Index::X, inner));
        assert_eq!(fingers.lock_scroll_drag(touch, dvec2(3.0, 2.0)), None);
        // Mostly sideways by the tap distance, so it stays sideways after it turns down.
        assert_eq!(fingers.lock_scroll_drag(touch, dvec2(6.0, -2.0)), Some(ScrollDirection::Along(Vec2Index::X)));
        assert_eq!(fingers.lock_scroll_drag(touch, dvec2(6.0, -80.0)), Some(ScrollDirection::Along(Vec2Index::X)));
        fingers.release_digit(touch);
        assert!(fingers.is_scroll_drag_owner(touch, Vec2Index::X, inner));
        assert!(fingers.is_scroll_drag_owner(touch, Vec2Index::Y, outer));
        assert_eq!(fingers.lock_scroll_drag(touch, dvec2(-4.0, 5.0)), Some(ScrollDirection::Free), "a diagonal start");
        fingers.release_digit(touch);
        // One scroller that goes both ways, like a map, pans freely.
        assert!(fingers.is_scroll_drag_owner(touch, Vec2Index::X, outer));
        assert!(fingers.is_scroll_drag_owner(touch, Vec2Index::Y, outer));
        assert_eq!(fingers.lock_scroll_drag(touch, dvec2(1.0, 7.0)), Some(ScrollDirection::Free));
    }

    #[test]
    fn a_trackpad_gesture_keeps_to_the_way_it_started() {
        let mut nest = Nest::new();
        // Like code in a page: the inner one only scrolls sideways and the outer one only down.
        nest.are_scrollable_x.set([true, false]);
        nest.are_scrollable.set([false, true]);
        assert_eq!(nest.scroll_by(ScrollPhase::Began, dvec2(3.0, 0.0)), (Some(INNER), Some(OUTER)));
        // Mostly sideways by the tap distance, so turning down later moves nothing but the code.
        assert_eq!(nest.scroll_by(ScrollPhase::Changed, dvec2(4.0, 1.0)), (Some(INNER), None));
        assert_eq!(nest.scroll_by(ScrollPhase::Changed, dvec2(0.0, 20.0)), (Some(INNER), None));
        assert_eq!(nest.scroll_by(ScrollPhase::Ended, dvec2(0.0, 0.0)), (Some(INNER), None));
        assert_eq!(nest.scroll_by(ScrollPhase::Momentum, dvec2(1.0, 15.0)), (Some(INNER), None));
        nest.scroll_by(ScrollPhase::MomentumEnded, dvec2(0.0, 0.0));
        // A diagonal start scrolls both ways for the whole gesture.
        assert_eq!(nest.scroll_by(ScrollPhase::Began, dvec2(4.0, 4.0)), (Some(INNER), Some(OUTER)));
        assert_eq!(nest.scroll_by(ScrollPhase::Changed, dvec2(4.0, 4.0)), (Some(INNER), Some(OUTER)));
        assert_eq!(nest.scroll_by(ScrollPhase::Changed, dvec2(0.0, 20.0)), (Some(INNER), Some(OUTER)));
        assert_eq!(nest.scroll_by(ScrollPhase::Changed, dvec2(20.0, 0.0)), (Some(INNER), Some(OUTER)));
        nest.scroll_by(ScrollPhase::Ended, dvec2(0.0, 0.0));

        // Over something that only scrolls down, a mostly sideways start never holds that back.
        nest.are_scrollable_x.set([false, false]);
        assert_eq!(nest.scroll_by(ScrollPhase::Began, dvec2(8.0, 1.0)), (None, Some(OUTER)));
        assert_eq!(nest.scroll_by(ScrollPhase::Changed, dvec2(0.0, 20.0)), (None, Some(OUTER)));
        nest.scroll_by(ScrollPhase::Ended, dvec2(0.0, 0.0));

        // One scroller that takes both ways, like a map, pans freely.
        nest.are_scrollable_x.set([true, false]);
        nest.are_scrollable.set([true, true]);
        assert_eq!(nest.scroll_by(ScrollPhase::Began, dvec2(8.0, 0.0)), (Some(INNER), Some(INNER)));
        assert_eq!(nest.scroll_by(ScrollPhase::Changed, dvec2(0.0, 20.0)), (Some(INNER), Some(INNER)));
    }
}
