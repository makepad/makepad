//! Touchpad gestures for the direct (evdev) backend, with macOS's feel: one
//! finger moves the pointer, two fingers scroll in smooth point deltas with
//! the same gesture phases macOS reports (`Touched`, `Began`, `Changed`,
//! `Ended`, then a decaying `Momentum` stream ending in `MomentumEnded`), and
//! two fingers spreading or closing pinch.
//!
//! Pure: `raw_input` feeds one [`PadFrame`] per evdev report (the contacts in
//! millimetres, the kernel's report time) and ticks the momentum from its
//! event loop, so every rule here is unit tested with recorded sequences.
//!
//! Rules:
//! * Two fingers must travel [`SCROLL_START_MM`] before a scroll begins; the
//!   direction of that travel locks the scroll to one axis unless it was
//!   diagonal. Sustained perpendicular travel frees the lock.
//! * Deltas are millimetres times a gain that grows with finger speed
//!   ([`GAIN_SLOW`]..[`GAIN_FAST`] points per mm): slow moves are precise,
//!   flicks travel far. No line steps.
//! * Natural scrolling (the default) moves the content with the fingers.
//! * Lifting the fingers ends the gesture (`Ended`) and coasts with the
//!   release velocity, measured over the last [`VELOCITY_WINDOW`] seconds of
//!   finger motion, decaying exponentially ([`MOMENTUM_TAU`]): a flick
//!   coasts for about 1-2 s. Any new touch on the pad stops the coast at once
//!   (`MomentumEnded`, then `Touched`).
//! * Palms (`MT_TOOL_PALM`) never count; a thumb resting in the bottom
//!   [`THUMB_ZONE_MM`] of the pad while another finger works does not either.
//! * The finger left on the pad after a two-finger gesture, three or more
//!   fingers, and a resync after dropped events are held still until every
//!   finger lifts, so the pointer never jumps.
//! * A clickpad press ([`PadGesture::click_button`]) is the secondary button
//!   with two fingers on the pad and the middle one with three (libinput's
//!   clickfinger); with one finger it is the secondary button in the bottom
//!   right corner ([`BUTTON_AREA_MM`] x [`BUTTON_AREA_RIGHT`]), else primary.
//!   The caller keeps that button until the release, so it drags.
//! * With tap to click, a short touch ([`TAP_TIME`], [`TAP_MOVE_MM`]) clicks:
//!   one finger primary, two secondary, three middle.

use crate::types::input;
use makepad_math::*;

/// Two-finger travel (mm) before a scroll begins: resting fingers and the
/// press of a two-finger click never scroll.
pub const SCROLL_START_MM: f64 = 1.0;
/// A start this many times longer along one axis locks the scroll to it.
pub const AXIS_LOCK_RATIO: f64 = 2.0;
/// Perpendicular travel (mm), net of half the along-axis travel, that frees
/// an axis lock: a deliberate diagonal frees it, a slightly slanted swipe not.
pub const AXIS_UNLOCK_MM: f64 = 6.0;
/// Change of finger distance (mm) that makes two fingers a pinch, if it
/// dominates their common travel by [`PINCH_DOMINANCE`].
pub const PINCH_START_MM: f64 = 4.0;
pub const PINCH_DOMINANCE: f64 = 1.5;
/// The clickpad's bottom band (mm) where a resting thumb is ignored...
pub const THUMB_ZONE_MM: f64 = 10.0;
/// ...as long as it stays this still (mm).
pub const THUMB_STILL_MM: f64 = 3.0;
/// Scroll points per finger millimetre, slow to fast, and the finger speeds
/// (mm/s) between which the gain eases from one to the other.
pub const GAIN_SLOW: f64 = 5.0;
pub const GAIN_FAST: f64 = 14.0;
pub const GAIN_SLOW_SPEED: f64 = 20.0;
pub const GAIN_FAST_SPEED: f64 = 250.0;
/// The release velocity is the scroll travelled over this window (s) before
/// the fingers lift; fingers that stopped before lifting release no coast.
pub const VELOCITY_WINDOW: f64 = 0.08;
/// A release slower than this (points/s) does not coast.
pub const MOMENTUM_MIN_SPEED: f64 = 60.0;
/// The coast's speed limit (points/s).
pub const MOMENTUM_MAX_SPEED: f64 = 6000.0;
/// The coast decays as `v(t) = v0 * exp(-t / tau)` (0.997 per ms, the same
/// rate as the touch-drag flick in `widgets::scroll_motion`)...
pub const MOMENTUM_TAU: f64 = 0.333;
/// ...and stops below this speed (points/s): 1.5 s after a 1000 pt/s flick.
pub const MOMENTUM_STOP_SPEED: f64 = 12.0;
/// The shortest momentum step (s); the event loop ticks every 8 ms anyway.
pub const MOMENTUM_MIN_STEP: f64 = 0.004;
/// The clickpad's software secondary button: the bottom band (mm)...
pub const BUTTON_AREA_MM: f64 = 12.0;
/// ...right of this fraction of the pad's width.
pub const BUTTON_AREA_RIGHT: f64 = 0.6;
/// Two contacts this far apart vertically (mm), the lower one in the bottom
/// fifth of the pad, are a thumb and a finger: a one-finger press.
pub const CLICKFINGER_SPREAD_MM: f64 = 30.0;
/// A tap: every finger lifts within this time (s) of the first touch...
pub const TAP_TIME: f64 = 0.18;
/// ...and no finger moved further than this (mm).
pub const TAP_MOVE_MM: f64 = 1.5;

/// One contact on the pad's surface, in millimetres from its top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PadContact {
    pub slot: usize,
    pub id: i32,
    pub pos: Vec2d,
    /// The pad reported it as a palm (`ABS_MT_TOOL_TYPE == MT_TOOL_PALM`).
    pub palm: bool,
}

/// One evdev report of the pad.
#[derive(Clone, Copy, Debug)]
pub struct PadFrame<'a> {
    /// The kernel's report time (s); only differences are used.
    pub time: f64,
    /// The contacts touching the surface (hovering ones are not listed).
    pub contacts: &'a [PadContact],
    /// The clickpad's button is held.
    pub button: bool,
}

/// A button the pad clicks: its `BTN_LEFT` offset and mouse button bit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PadButton {
    Primary = 0,
    Secondary = 1,
    Middle = 2,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PadOutput {
    /// Pointer motion in finger millimetres (the pointer curve is the caller's).
    Pointer(Vec2d),
    /// A scroll in points, oriented like `ScrollEvent::scroll` (positive y
    /// moves the view down, i.e. the content up).
    Scroll { delta: Vec2d, phase: u32 },
    Pinch { scale: f64, phase: u32 },
    /// A tap (tap to click): press and release of the button.
    Tap(PadButton),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Axis {
    Free,
    X,
    Y,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Idle,
    Pointer,
    /// Two fingers down, not yet a scroll or a pinch.
    Pending { travel: Vec2d, spread0: f64 },
    Scroll { axis: Axis, off_axis: f64 },
    Pinch,
    /// Still until every finger lifts.
    Held,
}

#[derive(Clone, Copy, Debug)]
struct Track {
    slot: usize,
    id: i32,
    start: Vec2d,
    prev: Vec2d,
    last: Vec2d,
    travel: f64,
}

/// A touch that may still be a tap.
#[derive(Clone, Copy, Debug)]
struct Tap {
    start: f64,
    fingers: usize,
    void: bool,
}

#[derive(Clone, Copy, Debug)]
struct Momentum {
    velocity: Vec2d,
    last_tick: Option<f64>,
}

pub struct PadGesture {
    /// The pad's surface (mm).
    size: Vec2d,
    tracks: Vec<Track>,
    /// The last report's contacts (palms included).
    contacts: Vec<PadContact>,
    tap: Option<Tap>,
    /// The contacts the gesture used in the last frame.
    active: Vec<(usize, i32)>,
    button: bool,
    centroid: Vec2d,
    spread: f64,
    last_time: f64,
    mode: Mode,
    /// Smoothed finger speed (mm/s); zero right after a new baseline.
    speed: f64,
    /// Cumulative emitted scroll (points) and its recent samples.
    scrolled: Vec2d,
    samples: Vec<(f64, Vec2d)>,
    momentum: Option<Momentum>,
}

impl PadGesture {
    pub fn new(size_mm: Vec2d) -> Self {
        Self {
            size: size_mm,
            tracks: Vec::new(),
            contacts: Vec::new(),
            tap: None,
            active: Vec::new(),
            button: false,
            centroid: dvec2(0.0, 0.0),
            spread: 0.0,
            last_time: 0.0,
            mode: Mode::Idle,
            speed: 0.0,
            scrolled: dvec2(0.0, 0.0),
            samples: Vec::new(),
            momentum: None,
        }
    }

    /// How many fingers the gesture counts now (palms and resting thumbs
    /// excluded).
    #[cfg(test)]
    pub fn finger_count(&self) -> usize {
        self.active.len()
    }

    /// The button a clickpad press clicks, from the contacts on the pad.
    pub fn click_button(&self) -> PadButton {
        let mut fingers: Vec<&PadContact> = self.contacts.iter().filter(|c| !c.palm).collect();
        fingers.sort_by(|a, b| a.pos.y.total_cmp(&b.pos.y));
        if let [upper, lower] = fingers[..] {
            if lower.pos.y > self.size.y * 0.8 && lower.pos.y - upper.pos.y > CLICKFINGER_SPREAD_MM {
                // A finger up the pad and the thumb pressing: the thumb counts.
                fingers.remove(0);
            }
        }
        match fingers.len() {
            0 => PadButton::Primary,
            1 => {
                let pos = fingers[0].pos;
                if pos.y > self.size.y - BUTTON_AREA_MM && pos.x > self.size.x * BUTTON_AREA_RIGHT {
                    PadButton::Secondary
                } else {
                    PadButton::Primary
                }
            }
            2 => PadButton::Secondary,
            _ => PadButton::Middle,
        }
    }

    pub fn is_coasting(&self) -> bool {
        self.momentum.is_some()
    }

    /// The pad's state was lost (dropped events): end whatever runs without a
    /// coast and hold everything still until every finger lifts.
    pub fn cancel(&mut self, out: &mut Vec<PadOutput>) {
        self.stop_momentum(out);
        self.end_gesture(0.0, false, out);
        self.mode = Mode::Held;
        self.tap = None;
        self.tracks.clear();
        self.active.clear();
    }

    /// One report; `tap` is tap to click.
    pub fn frame(&mut self, frame: &PadFrame, natural: bool, tap: bool, out: &mut Vec<PadOutput>) {
        // Every contact on the surface, new ones first: any new touch stops a
        // coast at once and is reported as a touch, like a finger landing on
        // a macOS trackpad.
        let mut new_touch = false;
        self.tracks.retain(|t| frame.contacts.iter().any(|c| c.slot == t.slot && c.id == t.id));
        for c in frame.contacts {
            match self.tracks.iter_mut().find(|t| t.slot == c.slot && t.id == c.id) {
                Some(t) => {
                    t.prev = t.last;
                    t.last = c.pos;
                    t.travel = t.travel.max((c.pos - t.start).length());
                }
                None => {
                    new_touch = true;
                    self.tracks.push(Track { slot: c.slot, id: c.id, start: c.pos, prev: c.pos, last: c.pos, travel: 0.0 });
                }
            }
        }
        if new_touch {
            // A touch that stops a coast is not a tap.
            let coasting = self.momentum.is_some();
            if self.contacts.is_empty() && self.mode != Mode::Held {
                self.tap = Some(Tap { start: frame.time, fingers: 0, void: !tap || coasting });
            }
            self.stop_momentum(out);
            out.push(PadOutput::Scroll { delta: dvec2(0.0, 0.0), phase: input::scroll::TOUCHED });
        }
        self.contacts.clear();
        self.contacts.extend_from_slice(frame.contacts);
        if let Some(t) = self.tap.as_mut() {
            if frame.button
                || frame.time - t.start > TAP_TIME
                || self.tracks.iter().any(|track| track.travel > TAP_MOVE_MM)
                || matches!(self.mode, Mode::Scroll { .. } | Mode::Pinch) {
                t.void = true;
            }
        }

        if frame.contacts.is_empty() {
            if let Some(t) = self.tap.take() {
                if !t.void {
                    match t.fingers {
                        1 => out.push(PadOutput::Tap(PadButton::Primary)),
                        2 => out.push(PadOutput::Tap(PadButton::Secondary)),
                        3 => out.push(PadOutput::Tap(PadButton::Middle)),
                        _ => {}
                    }
                }
            }
            self.end_gesture(frame.time, true, out);
            self.mode = Mode::Idle;
            self.active.clear();
            self.button = frame.button;
            return;
        }

        let active: Vec<&PadContact> = frame.contacts.iter().filter(|c| !c.palm && !self.is_thumb(c, frame.contacts)).collect();
        let ids: Vec<(usize, i32)> = active.iter().map(|c| (c.slot, c.id)).collect();
        let centroid = if active.is_empty() {
            dvec2(0.0, 0.0)
        } else {
            active.iter().fold(dvec2(0.0, 0.0), |sum, c| sum + c.pos) / active.len() as f64
        };
        let spread = if active.len() == 2 { (active[0].pos - active[1].pos).length() } else { 0.0 };
        if let Some(t) = self.tap.as_mut() {
            t.fingers = t.fingers.max(active.len());
        }

        if ids != self.active || frame.button != self.button {
            // A finger landed or lifted, or the button changed: end what was
            // running and take a new baseline, so nothing ever jumps.
            let lifted = ids.len() < 2 && !frame.button;
            let was = self.mode;
            self.end_gesture(frame.time, lifted, out);
            self.mode = if frame.button {
                Mode::Pointer
            } else {
                match (was, ids.len()) {
                    (Mode::Held, _) => Mode::Held,
                    (_, 0) => Mode::Idle,
                    (Mode::Idle | Mode::Pointer, 1) => Mode::Pointer,
                    (_, 1) => Mode::Held,
                    (_, 2) => Mode::Pending { travel: dvec2(0.0, 0.0), spread0: spread },
                    _ => Mode::Held,
                }
            };
            self.active = ids;
            self.button = frame.button;
            self.centroid = centroid;
            self.spread = spread;
            self.last_time = frame.time;
            self.speed = 0.0;
            return;
        }

        let delta = centroid - self.centroid;
        let dt = (frame.time - self.last_time).clamp(0.001, 0.1);
        let instant = delta.length() / dt;
        self.speed = if self.speed == 0.0 { instant } else { 0.5 * self.speed + 0.5 * instant };

        match self.mode {
            Mode::Pointer => {
                // With the button held, the finger that moves drags (the one
                // pressing the button stays put).
                let motion = if frame.button {
                    self.tracks.iter()
                        .filter(|t| self.active.contains(&(t.slot, t.id)))
                        .map(|t| t.last - t.prev)
                        .fold(dvec2(0.0, 0.0), |best, d| if d.length() > best.length() { d } else { best })
                } else {
                    delta
                };
                if motion.x != 0.0 || motion.y != 0.0 {
                    out.push(PadOutput::Pointer(motion));
                }
            }
            Mode::Pending { travel, spread0 } => {
                let travel = travel + delta;
                let pinch = spread - spread0;
                if pinch.abs() > PINCH_START_MM && pinch.abs() > PINCH_DOMINANCE * travel.length() && spread0 > 0.0 {
                    out.push(PadOutput::Pinch { scale: 1.0, phase: input::pinch::BEGIN });
                    out.push(PadOutput::Pinch { scale: spread / spread0, phase: input::pinch::UPDATE });
                    self.mode = Mode::Pinch;
                } else if travel.length() > SCROLL_START_MM {
                    let axis = if travel.y.abs() > AXIS_LOCK_RATIO * travel.x.abs() {
                        Axis::Y
                    } else if travel.x.abs() > AXIS_LOCK_RATIO * travel.y.abs() {
                        Axis::X
                    } else {
                        Axis::Free
                    };
                    self.scrolled = dvec2(0.0, 0.0);
                    self.samples.clear();
                    self.mode = Mode::Scroll { axis, off_axis: 0.0 };
                    // The whole start travel scrolls, so no motion is lost.
                    self.emit_scroll(frame.time, travel, axis, natural, input::scroll::BEGAN, out);
                } else {
                    self.mode = Mode::Pending { travel, spread0 };
                }
            }
            Mode::Scroll { mut axis, off_axis } => {
                let (along, across) = match axis {
                    Axis::X => (delta.x.abs(), delta.y.abs()),
                    Axis::Y => (delta.y.abs(), delta.x.abs()),
                    Axis::Free => (0.0, 0.0),
                };
                let off_axis = (off_axis + across - 0.5 * along).max(0.0);
                if off_axis > AXIS_UNLOCK_MM {
                    axis = Axis::Free;
                }
                self.mode = Mode::Scroll { axis, off_axis };
                self.emit_scroll(frame.time, delta, axis, natural, input::scroll::CHANGED, out);
            }
            Mode::Pinch => {
                if self.spread > 0.0 && spread > 0.0 && spread != self.spread {
                    out.push(PadOutput::Pinch { scale: spread / self.spread, phase: input::pinch::UPDATE });
                }
            }
            Mode::Held | Mode::Idle => {}
        }
        self.centroid = centroid;
        self.spread = spread;
        self.last_time = frame.time;
    }

    /// Advances a coast to the event loop's time `now` (s, any clock).
    pub fn tick(&mut self, now: f64, out: &mut Vec<PadOutput>) {
        let Some(momentum) = self.momentum.as_mut() else { return };
        let Some(last) = momentum.last_tick else {
            momentum.last_tick = Some(now);
            return;
        };
        let dt = now - last;
        if dt < MOMENTUM_MIN_STEP {
            return;
        }
        momentum.last_tick = Some(now);
        let decay = (-dt / MOMENTUM_TAU).exp();
        let step = momentum.velocity * (MOMENTUM_TAU * (1.0 - decay));
        momentum.velocity = momentum.velocity * decay;
        out.push(PadOutput::Scroll { delta: step, phase: input::scroll::MOMENTUM });
        if momentum.velocity.length() < MOMENTUM_STOP_SPEED {
            self.momentum = None;
            out.push(PadOutput::Scroll { delta: dvec2(0.0, 0.0), phase: input::scroll::MOMENTUM_ENDED });
        }
    }

    fn is_thumb(&self, c: &PadContact, all: &[PadContact]) -> bool {
        let in_zone = |slot: usize, id: i32| {
            self.tracks.iter().find(|t| t.slot == slot && t.id == id)
                .map_or(false, |t| t.start.y > self.size.y - THUMB_ZONE_MM)
        };
        let Some(track) = self.tracks.iter().find(|t| t.slot == c.slot && t.id == c.id) else { return false };
        in_zone(c.slot, c.id)
            && track.travel < THUMB_STILL_MM
            && all.iter().any(|o| (o.slot, o.id) != (c.slot, c.id) && !o.palm && !in_zone(o.slot, o.id))
    }

    fn emit_scroll(&mut self, time: f64, finger_mm: Vec2d, axis: Axis, natural: bool, phase: u32, out: &mut Vec<PadOutput>) {
        let gain = gain(self.speed);
        let mut delta = finger_mm * if natural { -gain } else { gain };
        match axis {
            Axis::X => delta.y = 0.0,
            Axis::Y => delta.x = 0.0,
            Axis::Free => {}
        }
        if phase == input::scroll::CHANGED && delta.x == 0.0 && delta.y == 0.0 {
            return;
        }
        self.scrolled += delta;
        self.samples.push((time, self.scrolled));
        while self.samples.first().is_some_and(|(t, _)| time - t > VELOCITY_WINDOW * 2.0) {
            self.samples.remove(0);
        }
        out.push(PadOutput::Scroll { delta, phase });
    }

    /// Ends a running scroll (coasting when the fingers `lifted`) or pinch.
    fn end_gesture(&mut self, time: f64, lifted: bool, out: &mut Vec<PadOutput>) {
        match self.mode {
            Mode::Scroll { .. } => {
                out.push(PadOutput::Scroll { delta: dvec2(0.0, 0.0), phase: input::scroll::ENDED });
                if lifted {
                    let velocity = release_velocity(&self.samples, time);
                    let speed = velocity.length();
                    if speed >= MOMENTUM_MIN_SPEED {
                        let velocity = if speed > MOMENTUM_MAX_SPEED { velocity * (MOMENTUM_MAX_SPEED / speed) } else { velocity };
                        self.momentum = Some(Momentum { velocity, last_tick: None });
                    }
                }
                self.samples.clear();
            }
            Mode::Pinch => out.push(PadOutput::Pinch { scale: 1.0, phase: input::pinch::END }),
            _ => {}
        }
        self.mode = Mode::Idle;
    }

    fn stop_momentum(&mut self, out: &mut Vec<PadOutput>) {
        if self.momentum.take().is_some() {
            out.push(PadOutput::Scroll { delta: dvec2(0.0, 0.0), phase: input::scroll::MOMENTUM_ENDED });
        }
    }
}

/// Points per finger millimetre at finger speed `speed` (mm/s).
pub fn gain(speed: f64) -> f64 {
    let t = ((speed - GAIN_SLOW_SPEED) / (GAIN_FAST_SPEED - GAIN_SLOW_SPEED)).clamp(0.0, 1.0);
    GAIN_SLOW + (GAIN_FAST - GAIN_SLOW) * t * t * (3.0 - 2.0 * t)
}

/// The scroll velocity (points/s) at lift time `lift`: the travel of the
/// samples inside the window before it, over the time from the first of them
/// to the lift, so a pause before lifting slows (or zeroes) the coast.
fn release_velocity(samples: &[(f64, Vec2d)], lift: f64) -> Vec2d {
    let window: Vec<&(f64, Vec2d)> = samples.iter().filter(|(t, _)| lift - t <= VELOCITY_WINDOW && *t <= lift).collect();
    let (Some(first), Some(last)) = (window.first(), window.last()) else { return dvec2(0.0, 0.0) };
    if window.len() < 2 {
        return dvec2(0.0, 0.0);
    }
    let dt = (lift - first.0).max(0.01);
    (last.1 - first.1) / dt
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ASUS clickpad on node 149 (ASUF1209:00 2808:0219): 148 x 96 mm.
    const PAD: Vec2d = Vec2d { x: 148.8, y: 96.8 };
    /// Its report period (~7 ms).
    const DT: f64 = 0.007;

    fn c(slot: usize, id: i32, x: f64, y: f64) -> PadContact {
        PadContact { slot, id, pos: dvec2(x, y), palm: false }
    }

    struct Run {
        pad: PadGesture,
        t: f64,
        out: Vec<PadOutput>,
        natural: bool,
        tap: bool,
    }

    impl Run {
        fn new() -> Self {
            Self { pad: PadGesture::new(PAD), t: 100.0, out: Vec::new(), natural: true, tap: false }
        }
        fn frame(&mut self, contacts: &[PadContact]) {
            self.frame_button(contacts, false);
        }
        fn frame_button(&mut self, contacts: &[PadContact], button: bool) {
            self.t += DT;
            let f = PadFrame { time: self.t, contacts, button };
            self.pad.frame(&f, self.natural, self.tap, &mut self.out);
        }
        /// Two fingers 20 mm apart moving by `step` mm per report.
        fn two_finger_swipe(&mut self, from: Vec2d, step: Vec2d, reports: usize) {
            for i in 0..reports {
                let p = from + step * i as f64;
                self.frame(&[c(0, 10, p.x, p.y), c(1, 11, p.x + 20.0, p.y)]);
            }
        }
        fn scrolls(&self) -> Vec<(Vec2d, u32)> {
            self.out.iter().filter_map(|o| match o {
                PadOutput::Scroll { delta, phase } => Some((*delta, *phase)),
                _ => None,
            }).collect()
        }
        fn phases(&self) -> Vec<u32> {
            let mut v: Vec<u32> = self.scrolls().into_iter().map(|(_, p)| p).collect();
            v.dedup();
            v
        }
        fn total(&self, phase: u32) -> Vec2d {
            self.scrolls().into_iter().filter(|(_, p)| *p == phase).fold(dvec2(0.0, 0.0), |s, (d, _)| s + d)
        }
        /// Ticks the coast at 120 Hz until it ends; returns its duration (s).
        fn coast(&mut self) -> f64 {
            let start = self.t;
            while self.pad.is_coasting() && self.t - start < 10.0 {
                self.t += 1.0 / 120.0;
                self.pad.tick(self.t, &mut self.out);
            }
            self.t - start
        }
    }

    #[test]
    fn two_finger_swipe_up_scrolls_naturally_with_phases_and_coasts() {
        let mut r = Run::new();
        // Fingers move up the pad (towards y = 0) at ~115 mm/s, then lift.
        r.two_finger_swipe(dvec2(40.0, 70.0), dvec2(0.0, -0.8), 40);
        r.frame(&[]);
        let finger = r.total(input::scroll::BEGAN) + r.total(input::scroll::CHANGED);
        // Natural: fingers up = content up = the view moves down (+y).
        assert!(finger.y > 0.0, "{finger:?}");
        assert_eq!(finger.x, 0.0, "a vertical start locks the x axis");
        assert_eq!(r.phases()[..4], [input::scroll::TOUCHED, input::scroll::BEGAN, input::scroll::CHANGED, input::scroll::ENDED]);
        assert!(r.pad.is_coasting());
        let duration = r.coast();
        assert!((0.8..2.5).contains(&duration), "coast lasted {duration} s");
        let coast = r.total(input::scroll::MOMENTUM);
        assert!(coast.y > 0.0 && coast.x == 0.0, "the coast continues the scroll: {coast:?}");
        assert_eq!(*r.phases().last().unwrap(), input::scroll::MOMENTUM_ENDED);
    }

    #[test]
    fn traditional_direction_is_the_opposite() {
        let mut natural = Run::new();
        natural.two_finger_swipe(dvec2(40.0, 70.0), dvec2(0.3, -0.8), 30);
        let mut traditional = Run::new();
        traditional.natural = false;
        traditional.two_finger_swipe(dvec2(40.0, 70.0), dvec2(0.3, -0.8), 30);
        let n = natural.total(input::scroll::CHANGED);
        let t = traditional.total(input::scroll::CHANGED);
        assert!(n.y > 0.0 && (n + t).length() < 1e-9, "{n:?} {t:?}");
    }

    #[test]
    fn horizontal_swipe_locks_to_x() {
        let mut r = Run::new();
        // Mostly right with a slight wobble down.
        r.two_finger_swipe(dvec2(20.0, 40.0), dvec2(1.0, 0.1), 30);
        let s = r.total(input::scroll::BEGAN) + r.total(input::scroll::CHANGED);
        assert!(s.x < 0.0, "natural: fingers right = content right = view left: {s:?}");
        assert_eq!(s.y, 0.0);
    }

    #[test]
    fn deliberate_diagonal_frees_the_lock() {
        let mut r = Run::new();
        r.two_finger_swipe(dvec2(40.0, 70.0), dvec2(0.0, -0.8), 6);
        // Then a turn to the side: sustained perpendicular travel.
        r.two_finger_swipe(dvec2(40.0, 66.0), dvec2(1.0, -0.2), 20);
        let last = r.scrolls().last().unwrap().0;
        assert!(last.x != 0.0, "the lock was freed: {last:?}");
    }

    #[test]
    fn a_small_start_does_not_scroll() {
        let mut r = Run::new();
        // Two fingers resting with sensor jitter (+-0.1 mm).
        for i in 0..30 {
            let j = if i % 2 == 0 { 0.1 } else { -0.1 };
            r.frame(&[c(0, 1, 50.0 + j, 50.0), c(1, 2, 70.0, 50.0 - j)]);
        }
        r.frame(&[]);
        assert_eq!(r.phases(), [input::scroll::TOUCHED]);
        assert!(!r.out.iter().any(|o| matches!(o, PadOutput::Pointer(_))));
    }

    #[test]
    fn fingers_that_stop_before_lifting_do_not_coast() {
        let mut r = Run::new();
        r.two_finger_swipe(dvec2(40.0, 70.0), dvec2(0.0, -0.8), 30);
        // Hold still for 150 ms, then lift.
        for _ in 0..21 {
            r.frame(&[c(0, 10, 40.0, 70.0 - 0.8 * 29.0), c(1, 11, 60.0, 70.0 - 0.8 * 29.0)]);
        }
        r.frame(&[]);
        assert!(r.phases().contains(&input::scroll::ENDED));
        assert!(!r.pad.is_coasting());
    }

    #[test]
    fn a_new_touch_stops_the_coast_at_once() {
        let mut r = Run::new();
        r.two_finger_swipe(dvec2(40.0, 70.0), dvec2(0.0, -1.2), 30);
        r.frame(&[]);
        for _ in 0..10 {
            r.t += 1.0 / 120.0;
            r.pad.tick(r.t, &mut r.out);
        }
        assert!(r.pad.is_coasting());
        r.out.clear();
        r.frame(&[c(2, 12, 70.0, 40.0)]);
        assert_eq!(r.phases(), [input::scroll::MOMENTUM_ENDED, input::scroll::TOUCHED]);
        assert!(!r.pad.is_coasting());
        r.out.clear();
        r.pad.tick(r.t + 0.1, &mut r.out);
        assert!(r.out.is_empty());
    }

    #[test]
    fn faster_flicks_coast_further() {
        let distance = |step: f64| {
            let mut r = Run::new();
            r.two_finger_swipe(dvec2(40.0, 80.0), dvec2(0.0, -step), 20);
            r.frame(&[]);
            r.coast();
            r.total(input::scroll::MOMENTUM).y
        };
        let (slow, fast) = (distance(0.4), distance(1.5));
        assert!(slow > 0.0 && fast > 2.0 * slow, "slow {slow} fast {fast}");
    }

    #[test]
    fn staggered_lift_ends_once_and_holds_the_last_finger() {
        let mut r = Run::new();
        r.two_finger_swipe(dvec2(40.0, 70.0), dvec2(0.0, -0.8), 30);
        // One finger lifts first, the other keeps sliding a little.
        r.frame(&[c(0, 10, 40.0, 46.0)]);
        r.frame(&[c(0, 10, 40.0, 44.0)]);
        r.frame(&[c(0, 10, 40.0, 42.0)]);
        r.frame(&[]);
        let ended = r.scrolls().iter().filter(|(_, p)| *p == input::scroll::ENDED).count();
        assert_eq!(ended, 1);
        assert!(!r.out.iter().any(|o| matches!(o, PadOutput::Pointer(_))), "the last finger never moves the pointer");
        assert!(r.pad.is_coasting());
    }

    #[test]
    fn one_finger_moves_the_pointer() {
        let mut r = Run::new();
        for i in 0..5 {
            r.frame(&[c(0, 1, 50.0 + i as f64, 50.0)]);
        }
        let moved: Vec2d = r.out.iter().filter_map(|o| match o { PadOutput::Pointer(d) => Some(*d), _ => None })
            .fold(dvec2(0.0, 0.0), |s, d| s + d);
        assert_eq!(moved, dvec2(4.0, 0.0));
        assert_eq!(r.phases(), [input::scroll::TOUCHED]);
    }

    #[test]
    fn a_palm_never_counts() {
        let mut r = Run::new();
        for i in 0..20 {
            let palm = PadContact { slot: 1, id: 2, pos: dvec2(120.0, 60.0 + i as f64 * 0.5), palm: true };
            r.frame(&[c(0, 1, 50.0 + i as f64, 50.0), palm]);
        }
        assert!(!r.phases().contains(&input::scroll::BEGAN), "one finger plus a palm is no scroll");
        assert!(r.out.iter().any(|o| matches!(o, PadOutput::Pointer(_))));
    }

    #[test]
    fn a_resting_thumb_leaves_the_finger_pointing() {
        let mut r = Run::new();
        for i in 0..20 {
            r.frame(&[c(0, 1, 50.0 + i as f64, 40.0), c(1, 2, 70.0, 92.0)]);
        }
        assert_eq!(r.pad.finger_count(), 1);
        assert!(!r.phases().contains(&input::scroll::BEGAN));
        assert!(r.out.iter().any(|o| matches!(o, PadOutput::Pointer(_))));
    }

    #[test]
    fn two_fingers_on_the_clickpad_count_two() {
        let mut r = Run::new();
        r.frame(&[c(0, 1, 50.0, 50.0), c(1, 2, 70.0, 50.0)]);
        assert_eq!(r.pad.finger_count(), 2);
    }

    #[test]
    fn clicking_mid_scroll_ends_without_coast_and_drags_with_the_moving_finger() {
        let mut r = Run::new();
        r.two_finger_swipe(dvec2(40.0, 70.0), dvec2(0.0, -1.0), 20);
        let at = 70.0 - 19.0;
        r.frame_button(&[c(0, 10, 40.0, at), c(1, 11, 60.0, at)], true);
        assert!(r.phases().contains(&input::scroll::ENDED));
        assert!(!r.pad.is_coasting());
        r.out.clear();
        // Finger 0 holds the button, finger 1 drags.
        r.frame_button(&[c(0, 10, 40.0, at), c(1, 11, 62.0, at + 1.0)], true);
        assert_eq!(r.out, [PadOutput::Pointer(dvec2(2.0, 1.0))]);
    }

    #[test]
    fn spreading_fingers_pinch_instead_of_scrolling() {
        let mut r = Run::new();
        for i in 0..20 {
            let s = i as f64 * 0.6;
            r.frame(&[c(0, 1, 60.0 - s, 50.0), c(1, 2, 80.0 + s, 50.0)]);
        }
        r.frame(&[]);
        let pinches: Vec<(f64, u32)> = r.out.iter().filter_map(|o| match o {
            PadOutput::Pinch { scale, phase } => Some((*scale, *phase)),
            _ => None,
        }).collect();
        assert_eq!(pinches.first().unwrap().1, input::pinch::BEGIN);
        assert_eq!(pinches.last().unwrap().1, input::pinch::END);
        let zoom: f64 = pinches.iter().map(|(s, _)| *s).product();
        assert!((zoom - (20.0 + 2.0 * 0.6 * 19.0) / 20.0).abs() < 1e-9, "zoom {zoom}");
        assert!(!r.phases().contains(&input::scroll::BEGAN));
    }

    #[test]
    fn three_fingers_hold_until_lift() {
        let mut r = Run::new();
        for i in 0..10 {
            let y = 60.0 - i as f64;
            r.frame(&[c(0, 1, 40.0, y), c(1, 2, 60.0, y), c(2, 3, 80.0, y)]);
        }
        r.frame(&[c(0, 1, 40.0, 50.0)]);
        r.frame(&[c(0, 1, 45.0, 50.0)]);
        assert_eq!(r.phases(), [input::scroll::TOUCHED]);
        assert!(!r.out.iter().any(|o| matches!(o, PadOutput::Pointer(_))));
        r.frame(&[]);
        r.frame(&[c(0, 4, 45.0, 50.0)]);
        r.frame(&[c(0, 4, 46.0, 50.0)]);
        assert!(r.out.iter().any(|o| matches!(o, PadOutput::Pointer(_))), "a fresh touch points again");
    }

    #[test]
    fn cancel_ends_the_scroll_and_holds() {
        let mut r = Run::new();
        r.two_finger_swipe(dvec2(40.0, 70.0), dvec2(0.0, -1.0), 20);
        r.pad.cancel(&mut r.out);
        assert_eq!(*r.phases().last().unwrap(), input::scroll::ENDED);
        assert!(!r.pad.is_coasting());
        r.out.clear();
        r.two_finger_swipe(dvec2(40.0, 50.0), dvec2(0.0, -1.0), 5);
        assert!(!r.phases().contains(&input::scroll::BEGAN));
    }

    fn taps(r: &Run) -> Vec<PadButton> {
        r.out.iter().filter_map(|o| match o { PadOutput::Tap(b) => Some(*b), _ => None }).collect()
    }

    #[test]
    fn two_fingers_click_secondary_three_middle() {
        let mut r = Run::new();
        r.frame(&[c(0, 10, 40.0, 50.0), c(1, 11, 60.0, 52.0)]);
        assert_eq!(r.pad.click_button(), PadButton::Secondary);
        r.frame(&[c(0, 10, 40.0, 50.0), c(1, 11, 60.0, 52.0), c(2, 12, 80.0, 50.0)]);
        assert_eq!(r.pad.click_button(), PadButton::Middle);
        // Both near the bottom edge: still two fingers.
        let mut r = Run::new();
        r.frame(&[c(0, 10, 40.0, 92.0), c(1, 11, 60.0, 93.0)]);
        assert_eq!(r.pad.click_button(), PadButton::Secondary);
    }

    #[test]
    fn one_finger_clicks_secondary_only_in_the_bottom_right_corner() {
        for (x, y, button) in [
            (140.0, 92.0, PadButton::Secondary),
            (95.0, 90.0, PadButton::Secondary),
            (140.0, 80.0, PadButton::Primary),
            (40.0, 92.0, PadButton::Primary),
            (74.0, 50.0, PadButton::Primary),
        ] {
            let mut r = Run::new();
            r.frame(&[c(0, 10, x, y)]);
            assert_eq!(r.pad.click_button(), button, "at {x}, {y}");
        }
    }

    #[test]
    fn a_thumb_far_below_the_finger_clicks_where_it_presses() {
        let mut r = Run::new();
        r.frame(&[c(0, 10, 60.0, 30.0), c(1, 11, 130.0, 92.0)]);
        assert_eq!(r.pad.click_button(), PadButton::Secondary, "thumb in the corner");
        let mut r = Run::new();
        r.frame(&[c(0, 10, 60.0, 30.0), c(1, 11, 40.0, 92.0)]);
        assert_eq!(r.pad.click_button(), PadButton::Primary, "thumb on the left");
    }

    #[test]
    fn a_palm_does_not_make_a_two_finger_click() {
        let mut r = Run::new();
        let palm = PadContact { palm: true, ..c(1, 11, 60.0, 52.0) };
        r.frame(&[c(0, 10, 40.0, 50.0), palm]);
        assert_eq!(r.pad.click_button(), PadButton::Primary);
    }

    #[test]
    fn taps_click_by_finger_count_when_enabled() {
        let mut r = Run::new();
        r.tap = true;
        r.frame(&[c(0, 10, 40.0, 50.0)]);
        r.frame(&[c(0, 10, 40.2, 50.1)]);
        r.frame(&[]);
        assert_eq!(taps(&r), [PadButton::Primary]);
        r.out.clear();
        // Two fingers, the second a report later.
        r.frame(&[c(0, 10, 40.0, 50.0)]);
        r.frame(&[c(0, 10, 40.0, 50.0), c(1, 11, 60.0, 50.0)]);
        r.frame(&[c(0, 10, 40.1, 50.0), c(1, 11, 60.0, 50.2)]);
        r.frame(&[c(1, 11, 60.0, 50.2)]);
        r.frame(&[]);
        assert_eq!(taps(&r), [PadButton::Secondary]);
        r.out.clear();
        for _ in 0..2 { r.frame(&[c(0, 10, 40.0, 50.0), c(1, 11, 60.0, 50.0), c(2, 12, 80.0, 50.0)]); }
        r.frame(&[]);
        assert_eq!(taps(&r), [PadButton::Middle]);
    }

    #[test]
    fn no_tap_when_off_long_moved_or_clicked() {
        let mut r = Run::new();
        r.frame(&[c(0, 10, 40.0, 50.0), c(1, 11, 60.0, 50.0)]);
        r.frame(&[]);
        assert!(taps(&r).is_empty(), "tap to click off");
        r.tap = true;
        for _ in 0..30 { r.frame(&[c(0, 10, 40.0, 50.0)]); }
        r.frame(&[]);
        assert!(taps(&r).is_empty(), "held 0.2 s");
        for i in 0..5 { r.frame(&[c(0, 10, 40.0 + i as f64, 50.0)]); }
        r.frame(&[]);
        assert!(taps(&r).is_empty(), "moved 4 mm");
        r.frame(&[c(0, 10, 40.0, 50.0)]);
        r.frame_button(&[c(0, 10, 40.0, 50.0)], true);
        r.frame_button(&[c(0, 10, 40.0, 50.0)], false);
        r.frame(&[]);
        assert!(taps(&r).is_empty(), "a physical click");
    }

    #[test]
    fn a_touch_that_stops_a_coast_is_not_a_tap() {
        let mut r = Run::new();
        r.tap = true;
        r.two_finger_swipe(dvec2(40.0, 70.0), dvec2(0.0, -0.8), 40);
        r.frame(&[]);
        assert!(r.pad.is_coasting());
        r.frame(&[c(0, 10, 40.0, 50.0)]);
        r.frame(&[]);
        assert!(taps(&r).is_empty());
    }

    #[test]
    fn gain_eases_from_slow_to_fast() {
        assert_eq!(gain(0.0), GAIN_SLOW);
        assert_eq!(gain(1000.0), GAIN_FAST);
        assert!(gain(100.0) > GAIN_SLOW && gain(100.0) < GAIN_FAST);
    }
}
