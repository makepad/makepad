//! Phone shell state and geometry. Application viewports remain stable while
//! their compositor surfaces move between home, foreground and the task viewer.
use crate::{desktop::DesktopStyle, hub::ClientId, mobile_tiles::{HomeOrder, HomeTiles, Slot}};
use makepad_widgets::makepad_platform::SafeAreaInsets;
use makepad_widgets::*;
use std::collections::HashMap;

/// What the phone shell draws AROUND the apps, decided once for the build
/// (`PhoneChrome::for_host` at startup). The desktop's iOS/Android skin is
/// a whole phone: a fake status bar (clock, wifi, battery), the island or
/// camera dot, the home indicator, and the simulator's controls strip
/// (style menu, Desktop, appearance, rotate). A real phone has all of that
/// from its OS — it draws none of it and lays out against the window's
/// real safe-area insets (the status bar / island above, the home
/// indicator below), which the platform reports with every window
/// geometry and the shell keeps current (`set_insets`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum PhoneChrome {
    /// A desktop window playing a phone.
    #[default]
    Simulated,
    /// The phone itself, with the insets its OS reported.
    Device { insets: SafeAreaInsets },
}

impl PhoneChrome {
    pub fn for_host(device: bool, insets: SafeAreaInsets) -> Self {
        if device { PhoneChrome::Device { insets } } else { PhoneChrome::Simulated }
    }
    /// The fake status bar: clock, wifi, battery, the island / camera dot.
    pub fn fake_status(&self) -> bool { matches!(self, PhoneChrome::Simulated) }
    /// The fake home indicator pill.
    pub fn fake_indicator(&self) -> bool { matches!(self, PhoneChrome::Simulated) }
    /// The desktop simulator's controls strip above the phone.
    pub fn controls_strip(&self) -> bool { matches!(self, PhoneChrome::Simulated) }
    /// A new window geometry: the device's insets follow it; the desktop
    /// skin's fake bars never change.
    pub fn set_insets(&mut self, insets: SafeAreaInsets) {
        if let PhoneChrome::Device { insets: current } = self { *current = insets; }
    }
    /// What the top of the screen reserves: the fake status bar (42 in
    /// portrait, 24 in landscape) or the real top inset.
    pub fn top_reserve(&self, screen: Rect) -> f64 {
        match self {
            PhoneChrome::Simulated => if screen.size.x > screen.size.y { 24.0 } else { 42.0 },
            PhoneChrome::Device { insets } => insets.top,
        }
    }
    /// What the bottom reserves: the fake home indicator strip (24) or the
    /// real bottom inset.
    pub fn bottom_reserve(&self, _screen: Rect) -> f64 {
        match self {
            PhoneChrome::Simulated => 24.0,
            // Android: the WM's own home strip, with its own pill — the OS's
            // handle leaves the WM for the system launcher. Where the surface
            // runs under the navigation bar (edge to edge) the OS owns its
            // gesture band there (no app can defer or exclude it), so the
            // strip sits above that band; a surface that ends above the
            // system bars (insets.bottom 0) has no band of the OS's inside it.
            PhoneChrome::Device { insets } if cfg!(target_os = "android") => {
                let os_band = if insets.bottom > 0.5 { insets.bottom.max(ANDROID_SYSTEM_GESTURE_BAND) } else { 0.0 };
                os_band + ANDROID_SHELL_STRIP
            }
            // Real phones with gesture nav often report 0 here (the OS
            // already reserved the nav strip). Keep a tappable home bar
            // inside the app so we are not stuck in an in-process tile.
            PhoneChrome::Device { insets } => insets.bottom.max(28.0),
        }
    }
    /// Where the WM draws its own home pill: the Android phone's shell
    /// strip above the OS gesture band, else the middle of the bottom
    /// reserve. `None` where the OS draws the only indicator (iOS).
    pub fn shell_pill(&self, screen: Rect) -> Option<Rect> {
        let bottom = screen.pos.y + screen.size.y - self.bottom_reserve(screen);
        let (top, h, pill) = match self {
            PhoneChrome::Simulated => (bottom, self.bottom_reserve(screen), 4.0),
            PhoneChrome::Device { .. } if cfg!(target_os = "android") => (bottom, ANDROID_SHELL_STRIP, 5.0),
            PhoneChrome::Device { .. } => return None,
        };
        Some(Rect { pos: dvec2(screen.pos.x + screen.size.x * 0.5 - 60.0, top + (h - pill) * 0.5), size: dvec2(120.0, pill) })
    }
    /// The shell's home gesture zone: the bottom reserve plus a few points
    /// of slack. A press here is the shell's (a tap goes Home, a swipe up
    /// goes Home or to Recents), never the app's — on the phone that is
    /// the strip under the real home indicator, whose first swipe the OS
    /// defers to the app (`Cx::defer_system_gestures`).
    pub fn bottom_zone(&self, screen: Rect, p: Vec2d) -> bool {
        p.y > screen.pos.y + screen.size.y - self.bottom_reserve(screen) - 4.0
    }
}

/// The Android system's own bottom gesture band (its mandatory system
/// gestures inset: 78 px at 2.44x = 32 pt on the Pixel 11 Pro XL).
pub const ANDROID_SYSTEM_GESTURE_BAND: f64 = 32.0;
/// The WM's home strip above it: tall enough to find with a thumb.
pub const ANDROID_SHELL_STRIP: f64 = 32.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PhoneScreen { #[default] Home, App, Recents, Drawer }

#[derive(Clone, Debug, PartialEq)]
pub enum PhoneHit {
    App(String), Card(ClientId), Home, Recents, Drawer, Back,
    Rotate, Style, Appearance, Desktop, Key(String), Shift, Symbols, HideKeyboard,
    ClearSearch, CancelSearch,
    /// Edit mode's "Done" pill.
    Done,
    /// The home screen's look switcher: 0 light, 1 dark, 2 the iOS skin.
    Look(u8),
}

/// How far past its ends a paged scroller or the library follows the
/// finger: the rubber band starts at this share of the overshoot and
/// saturates toward [`RUBBER_LIMIT`] points.
pub const RUBBER: f64 = 0.35;
pub const RUBBER_LIMIT: f64 = 64.0;
/// A sideways flick this fast (points per second), after [`FLICK_TRAVEL`]
/// points of travel, flips the home pages one page its way whatever the
/// distance was.
pub const PAGE_FLICK_SPEED: f64 = 480.0;
/// The same for the switcher's cards.
pub const CARD_FLICK_SPEED: f64 = 650.0;
/// A vertical flick this fast (points per second) completes the drawer's
/// open or close.
pub const DRAWER_FLICK_SPEED: f64 = 600.0;
pub const FLICK_TRAVEL: f64 = 16.0;
/// A slow release settles where the finger's speed would carry the page
/// in this long: the nearest page to that projection.
pub const PROJECT_SECS: f64 = 0.12;
/// Gesture ownership: a finger is a drag once it travels this far, and
/// takes an axis once one leads the other by [`AXIS_LOCK`].
pub const TOUCH_SLOP: f64 = 8.0;
pub const AXIS_LOCK: f64 = 1.2;
/// The release speed is the finger's over this last stretch of time; a
/// finger that rested this long before lifting was not moving at all.
pub const VELOCITY_WINDOW: f64 = 0.08;
pub const VELOCITY_REST: f64 = 0.12;

/// The motion every surface of the shell settles with: a critically damped
/// spring (mass 1, stiffness 900, damping 60 — 30 per second), seeded with
/// the finger's speed at release so a flick carries on and a slow release
/// eases from rest. About 0.3 s from rest to rest over any distance.
pub const SPRING_OMEGA: f64 = 30.0;

/// One exact step of the spring toward `target` over `dt`: the position
/// and velocity after it (the closed-form solution, so no step size can
/// make it overshoot or wobble).
pub fn spring_step(x: f64, v: f64, target: f64, dt: f64) -> (f64, f64) {
    let w = SPRING_OMEGA;
    let e0 = x - target;
    let b = v + w * e0;
    let decay = (-w * dt).exp();
    let e = (e0 + b * dt) * decay;
    let v = (v - w * b * dt) * decay;
    (target + e, v)
}

/// Rubber band, in points past the ends: the finger's own travel inside
/// `[0, max]`; beyond it a band that starts at [`RUBBER`] of the overshoot
/// and never gives more than [`RUBBER_LIMIT`] points.
pub fn rubber_band(raw: f64, max: f64, unit: f64) -> f64 {
    let band = |over: f64| {
        let d = over.abs() * unit.max(1.0);
        over.signum() * (RUBBER * d / (1.0 + RUBBER * d / RUBBER_LIMIT)) / unit.max(1.0)
    };
    if raw < 0.0 {
        band(raw)
    } else if raw > max {
        max + band(raw - max)
    } else {
        raw
    }
}

/// A paged scroller the finger drives — the switcher's cards, the home
/// pages and the drawer. While dragging the page follows the finger 1:1
/// (rubber band past the ends); on release a flick flips exactly one page
/// its way, a slower release settles on the page nearest to where its
/// speed projects, and the spring carries the finger's speed into the
/// settle. A settled scroller requests no frames (`step` returns false).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paging {
    /// The page shown, fractional between pages.
    pub page: f64,
    target: f64,
    /// Pages per second: the spring's speed (seeded by the release).
    velocity: f64,
    /// Points per page, from the last drag: what "a quarter point" is in
    /// pages when the spring decides it has arrived.
    unit: f64,
    /// A drag in flight: the page it started on, the finger's travel so far.
    drag: Option<(f64, f64)>,
}

impl Default for Paging {
    fn default() -> Self {
        Paging { page: 0.0, target: 0.0, velocity: 0.0, unit: 400.0, drag: None }
    }
}

impl Paging {
    fn max(count: usize) -> f64 {
        count.saturating_sub(1) as f64
    }
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }
    pub fn settled(&self) -> bool {
        self.drag.is_none() && self.page == self.target && self.velocity == 0.0
    }
    /// The finger lands; the page it holds is the drag's origin. A page
    /// still settling stops where it is, under the finger.
    pub fn drag_begin(&mut self) {
        self.drag = Some((self.page, 0.0));
        self.target = self.page;
        self.velocity = 0.0;
    }
    /// The finger moved `dx` points (right positive) over pages `width`
    /// points wide: the page follows, the ends rubber-band.
    pub fn drag_move(&mut self, dx: f64, width: f64, count: usize) {
        let Some((start, travel)) = self.drag.as_mut() else { return };
        *travel += dx;
        let width = width.max(1.0);
        let raw = *start - *travel / width;
        self.unit = width;
        self.page = rubber_band(raw, Self::max(count), width);
    }
    /// The finger lifts at `vx` points per second (right positive): a flick
    /// (faster than `flick_speed` after [`FLICK_TRAVEL`] points) flips one
    /// page its way; anything slower settles on the page nearest to where
    /// its speed projects, never more than one page from where the drag
    /// began. The spring starts at the finger's speed.
    pub fn drag_end(&mut self, vx: f64, width: f64, count: usize, flick_speed: f64) {
        let Some((start, travel)) = self.drag.take() else { return };
        let width = width.max(1.0);
        self.unit = width;
        let flick = travel.abs() >= FLICK_TRAVEL && vx.abs() > flick_speed;
        let target = if flick && vx < 0.0 {
            self.page.floor() + 1.0
        } else if flick {
            self.page.ceil() - 1.0
        } else {
            (self.page - vx * PROJECT_SECS / width).round().clamp(start.round() - 1.0, start.round() + 1.0)
        };
        self.target = target.clamp(0.0, Self::max(count));
        self.velocity = -vx / width;
    }
    /// The drag is taken away (not released): the page settles on the
    /// nearest page from rest, no flick, no projection.
    pub fn cancel_drag(&mut self, count: usize) {
        if self.drag.take().is_some() {
            self.target = self.page.round().clamp(0.0, Self::max(count));
            self.velocity = 0.0;
        }
    }
    /// A wheel or key step: one page, eased.
    pub fn flip(&mut self, by: i64, count: usize) {
        if self.drag.is_some() {
            return;
        }
        self.target = (self.target.round() + by as f64).clamp(0.0, Self::max(count));
    }
    pub fn set(&mut self, page: f64) {
        self.page = page;
        self.target = page;
        self.velocity = 0.0;
        self.drag = None;
    }
    /// The page it is going to (or holds).
    pub fn target(&self) -> f64 {
        self.target
    }
    /// Go to `page`, eased; a drag in flight is dropped.
    pub fn set_target(&mut self, page: f64, count: usize) {
        self.drag = None;
        self.target = page.clamp(0.0, Self::max(count));
    }
    /// One spring step toward the settled page; true while still moving.
    /// It rests once it is within a quarter point and slower than five
    /// points a second.
    pub fn step(&mut self, dt: f64) -> bool {
        if self.drag.is_some() || self.settled() {
            return false;
        }
        (self.page, self.velocity) = spring_step(self.page, self.velocity, self.target, dt);
        if (self.target - self.page).abs() * self.unit < 0.25 && self.velocity.abs() * self.unit < 5.0 {
            self.page = self.target;
            self.velocity = 0.0;
        }
        !self.settled()
    }
}

/// A finger resting this long without moving more than
/// [`LONG_PRESS_SLOP`] is a long press: on a home icon it opens edit mode.
pub const LONG_PRESS_SECS: f64 = 0.5;
pub const LONG_PRESS_SLOP: f64 = 8.0;
/// Edit mode's jiggle: every icon rocks this far either way, at this rate,
/// each with its own phase.
pub const JIGGLE_DEGREES: f64 = 2.0;
pub const JIGGLE_HZ: f64 = 1.7;
/// An app opens out of (and closes into) its icon or tile in this long:
/// the openness ease, and the fade a crossfade tile app matches.
pub const OPEN_SECS: f64 = 0.25;
/// How long after the last interaction the phone keeps painting (the
/// wallpaper's drift, a settling ease) before it rests.
pub const INTERACTION_TAIL_SECS: f64 = 1.0;
/// The wallpaper comes back up to speed over this long when touched…
pub const WALLPAPER_RAMP_SECS: f64 = 0.6;
/// …and its speed decays with this time constant once the phone rests
/// (about 5 s to a standstill).
pub const WALLPAPER_SETTLE_SECS: f64 = 1.2;

#[derive(Clone)]
pub struct PhoneGesture {
    pub start: Vec2d,
    pub last: Vec2d,
    pub time: f64,
    /// The farthest the finger has been from where it landed.
    pub max_travel: f64,
    /// The long press fired (edit mode opened, or a drag began).
    pub long_pressed: bool,
    /// Decided once the finger has travelled: a sideways pan of the home
    /// pages (true) or not (false).
    pub pan: Option<bool>,
    /// When the finger last moved, and how fast it was going then (points
    /// per second, up is negative): what tells a flick from a swipe-and-hold.
    pub last_time: f64,
    pub vy: f64,
    pub vx: f64,
    /// The finger's recent path, [`VELOCITY_WINDOW`] and a little more:
    /// what the release speed is measured over.
    samples: Vec<(f64, Vec2d)>,
    /// Where the finger last settled: `last_time` moves only once the
    /// finger leaves this spot by more than [`LONG_PRESS_SLOP`], so a
    /// twitch at the end of a hold does not undo the hold.
    rest_at: Vec2d,
    /// The finger travelled past [`TOUCH_SLOP`]: this is a drag from here
    /// to its release, never a tap.
    pub committed: bool,
    pub hit: Option<PhoneHit>,
    pub bottom: bool,
    pub edge: bool,
    pub screen: PhoneScreen,
}

impl PhoneGesture {
    pub fn new(p: Vec2d, time: f64, hit: Option<PhoneHit>, bottom: bool, edge: bool, screen: PhoneScreen) -> Self {
        PhoneGesture { start: p, last: p, time, max_travel: 0.0, long_pressed: false, pan: None, last_time: time, vy: 0.0, vx: 0.0, samples: vec![(time, p)], rest_at: p, committed: false, hit, bottom, edge, screen }
    }
    /// The finger is at `p` at `time`: its speed is the travel over the last
    /// [`VELOCITY_WINDOW`] seconds of samples, measured by time rather
    /// than per event, so a fast mouse and a 120 Hz finger agree.
    pub fn sample(&mut self, p: Vec2d, time: f64) {
        if (p - self.rest_at).length() > LONG_PRESS_SLOP {
            self.rest_at = p;
            self.last_time = time;
        }
        // A pause longer than a rest breaks the path: what came before it
        // says nothing about the speed now.
        if self.samples.last().is_some_and(|(t, _)| time - *t > VELOCITY_REST) {
            self.samples.clear();
        }
        self.samples.push((time, p));
        // Keep one sample at or before the window's start, so the window
        // always has a boundary to measure from.
        let start = time - VELOCITY_WINDOW;
        while self.samples.len() > 2 && self.samples[1].0 <= start {
            self.samples.remove(0);
        }
        let (t0, p0) = self.samples[0];
        let from = if t0 < start && self.samples.len() > 1 {
            // Interpolate the finger's place at the window's start.
            let (t1, p1) = self.samples[1];
            let k = ((start - t0) / (t1 - t0).max(1e-9)).clamp(0.0, 1.0);
            (start, p0 + (p1 - p0) * k)
        } else {
            (t0, p0)
        };
        let dt = time - from.0;
        let v = if dt > 0.004 { (p - from.1) / dt } else { dvec2(0.0, 0.0) };
        self.vx = v.x;
        self.vy = v.y;
    }
    /// The speed at release `now`: nothing if the finger rested before it
    /// lifted.
    pub fn release_velocity(&self, now: f64) -> Vec2d {
        if now - self.last_time > VELOCITY_REST { dvec2(0.0, 0.0) } else { dvec2(self.vx, self.vy) }
    }
    /// The axis this drag owns, decided once: past [`TOUCH_SLOP`], the
    /// first axis to lead the other by [`AXIS_LOCK`] (or the longer one
    /// once the finger is well past the slop). `Some(true)` = the
    /// `primary` axis (x when `primary_x`), `None` = not decided yet.
    pub fn lock_axis(&mut self, delta: Vec2d, primary_x: bool) -> Option<bool> {
        if self.pan.is_none() && delta.length() > TOUCH_SLOP {
            let (a, b) = if primary_x { (delta.x.abs(), delta.y.abs()) } else { (delta.y.abs(), delta.x.abs()) };
            if a > b * AXIS_LOCK { self.pan = Some(true); }
            else if b > a * AXIS_LOCK || delta.length() > TOUCH_SLOP * 3.0 { self.pan = Some(a >= b); }
        }
        self.pan
    }
    /// The finger moved to `p`: the farthest travel is remembered, so a
    /// finger that wandered and came back is no long press.
    pub fn track(&mut self, p: Vec2d) {
        self.max_travel = self.max_travel.max((p - self.start).length());
        self.committed |= self.max_travel > TOUCH_SLOP;
    }
    /// A long press is due: held [`LONG_PRESS_SECS`] within the slop and
    /// not fired yet. The caller marks it fired.
    pub fn long_press_due(&self, now: f64) -> bool {
        !self.long_pressed && self.max_travel <= LONG_PRESS_SLOP && now - self.time >= LONG_PRESS_SECS
    }
}

/// An icon in flight during edit mode: which one, the slot it holds right
/// now (the order already reflects it), where the finger is and how far
/// the finger is from the icon's centre, so the icon never jumps under it.
#[derive(Clone, Debug, PartialEq)]
pub struct IconDrag {
    pub id: String,
    pub slot: Slot,
    pub pos: Vec2d,
    pub grab: Vec2d,
}

/// Edit mode ("jiggle"): every icon rocks, the pressed one lifts and
/// follows the finger, the others shift aside live. An animation that
/// lasts exactly as long as the mode: `since` is its clock's origin and
/// `lift` eases the held icon up and back down.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HomeEdit {
    pub active: bool,
    pub since: f64,
    pub drag: Option<IconDrag>,
    pub lift: f64,
}

impl HomeEdit {
    /// The icon at `index`'s tilt at `now`, in degrees.
    pub fn jiggle(&self, index: usize, now: f64) -> f64 {
        if !self.active { return 0.0; }
        let t = now - self.since;
        JIGGLE_DEGREES * (t * JIGGLE_HZ * std::f64::consts::TAU + index as f64 * 1.9).sin()
    }
}

/// How long a finger must rest before release for an upward bottom swipe
/// to mean "hold" (the switcher) rather than a flick home.
pub const SWIPE_HOLD_SECS: f64 = 0.15;
/// Points per second upward at release that make a flick.
pub const SWIPE_FLICK_SPEED: f64 = 500.0;
/// Android: the travel up a lifted app needs to go home (Quickstep's
/// `motion_pause_detector_min_displacement_from_app`, 36 dp).
pub const LIFT_HOME_MIN: f64 = 36.0;
/// Android: a lifted app whose finger moves slower than this (Quickstep's
/// motion pause, 0.0285 dp/ms) has paused — the switcher.
pub const LIFT_PAUSE_SPEED: f64 = 28.5;

/// Where an upward swipe from the bottom edge goes, the way a phone decides
/// it: a FLICK (still moving fast at release, or released far up the
/// screen, or short and quick) goes home; a swipe the finger HELD before
/// letting go opens the switcher; from the switcher itself any upward
/// swipe goes home; on Android's home page it opens the drawer. `None`
/// when the swipe is too short to mean anything (a tap on the strip is
/// decided by the caller).
pub fn bottom_swipe_target(
    from: PhoneScreen,
    android: bool,
    delta: Vec2d,
    screen_height: f64,
    duration: f64,
    held: bool,
    vy: f64,
) -> Option<PhoneHit> {
    // Android (Quickstep `calculateEndTarget`): a lifted app goes home once
    // it travelled 36 dp up, to the switcher if it paused, else drops back.
    if android && from == PhoneScreen::App {
        if delta.y > -LIFT_HOME_MIN {
            return None;
        }
        return Some(if held { PhoneHit::Recents } else { PhoneHit::Home });
    }
    if delta.y >= -45.0 {
        return None;
    }
    // On Android's Home the strip is where multitasking lives: any swipe
    // up from it (a flick too — Home is already home) opens the running
    // apps. The all-apps drawer is a swipe up anywhere ABOVE the strip.
    if from == PhoneScreen::Home && android {
        return Some(PhoneHit::Recents);
    }
    if from == PhoneScreen::Recents {
        return Some(PhoneHit::Home);
    }
    // A finger that rested before lifting asked for the switcher, however
    // far it went; otherwise a flick, a long swipe or a short quick one
    // goes home.
    if held {
        return Some(PhoneHit::Recents);
    }

    let far = delta.y < -screen_height * 0.33;
    let quick = duration < 0.30 && delta.y < -100.0;
    let flick = vy < -SWIPE_FLICK_SPEED && delta.y <= -48.0;
    Some(if far || flick || quick { PhoneHit::Home } else { PhoneHit::Recents })
}

#[derive(Clone)]
pub struct PhoneState {
    pub clock: String,
    pub wallpaper_time: f64,
    /// The wallpaper's own animation clock: it runs at `wallpaper_speed`,
    /// which is 1 while the phone is in use and decays to a standstill
    /// after (`step_wallpaper`) — then the phone paints nothing.
    pub wallpaper_phase: f64,
    pub wallpaper_speed: f64,
    /// Touch events carry the OS's own clock (Android: its uptime), the
    /// phone's frames and `last_interaction` the app's: this is added to a
    /// touch's time (set at each touch start). A mismatch left the
    /// interaction tail always "just now" — the phone never rested — and a
    /// long press never due.
    pub touch_time_offset: f64,
    /// The frame clock minus `seconds_since_app_start`, from the last frame.
    pub frame_clock_delta: f64,
    pub screen: PhoneScreen,
    pub client: Option<ClientId>,
    pub order: Vec<ClientId>,
    pub openness: f64,
    pub overview: f64,
    /// The springs' speeds under `openness` and `overview` (per second).
    pub openness_v: f64,
    pub overview_v: f64,
    /// Where the open app zooms out of: the icon or tile that was tapped
    /// (screen rect), for the client it opened. `launch_from` holds the
    /// tapped rect until the client is activated; `origin_in_drawer` says
    /// the tap was in the drawer, which is not there to close back into.
    pub launch_from: Option<(Rect, bool)>,
    pub origin: Option<(ClientId, Rect, bool)>,
    /// The switcher's cards, paged by the finger.
    pub cards: Paging,
    /// The home pager: the home page(s) then the App Library as the last
    /// page, panned by the finger (the drawer on Android, driven by a
    /// vertical drag through the same pager).
    pub pager: Paging,
    pub home_pages: usize,
    /// The App Library / drawer's arrival, derived from the pager every
    /// step: 0 = the last home page, 1 = the library, the finger's own
    /// value in between (rubber past either end).
    pub drawer: f64,
    /// The home page's icon order and the dock, as arranged (mobile_tiles
    /// `HomeOrder`); `home_order_loaded` once storage answered (or had
    /// nothing), so a first-run default is never saved over a real one.
    pub home: HomeOrder,
    pub home_order_loaded: bool,
    pub edit: HomeEdit,
    /// When the person last touched the phone: the paint clock keeps
    /// running [`INTERACTION_TAIL_SECS`] past it, then rests.
    pub last_interaction: f64,
    pub dismiss_y: f64,
    pub gesture: Option<PhoneGesture>,
    /// Native touch owned by shell navigation; other fingers cannot replace it.
    pub touch: Option<u64>,
    pub keyboard: f64,
    pub keyboard_target: f64,
    pub keyboard_sent_height: f64,
    pub keyboard_client: Option<ClientId>,
    pub search_query: String,
    pub search_focused: bool,
    pub search_scroll: f64,
    pub ime: HashMap<ClientId, makepad_platform::ime::HostedImeState>,
    pub shift: bool,
    pub symbols: bool,
    pub desktop_size: Option<Vec2d>,
    pub desktop_clients: Vec<ClientId>,
    pub desktop_style: DesktopStyle,
    pub viewport: Rect,
    /// What the shell draws around the apps (see [`PhoneChrome`]).
    pub chrome: PhoneChrome,
    /// The desk painted the device's status-bar band from the foreground
    /// app's own frame (its top rows stretched up), so a dark app never
    /// sits under a light strip; the overlay then leaves the band alone.
    pub band_from_app: bool,
    /// The home page's live app tiles (mobile_tiles.rs): which client shows
    /// which tile and in which face.
    pub tiles: HomeTiles,
    /// The open app held by a bottom swipe: where the finger put it (see
    /// [`lift_rect`]). While the finger is down `blend` is 1 and the app is
    /// drawn exactly there; after release it springs to 0, handing the app
    /// over to the openness/overview springs without a jump.
    pub lift: Option<Lift>,
    /// The Android skin: its motion follows the Pixel Launcher
    /// (`launcher_motion`); the fields below drive it.
    pub android: bool,
    /// Device pixels per point (the spec's px speeds are converted with it).
    pub density: f64,
    /// An app opening from its icon: seconds since the tap (500 ms, then
    /// cleared). `openness` follows it linearly.
    pub open_elapsed: Option<f64>,
    /// A lifted app released home, flying into its icon.
    pub flight: Option<crate::launcher_motion::HomeFlight>,
    /// The home screen revealing after a swipe home: seconds since.
    pub reveal_elapsed: Option<f64>,
    /// Recents' neighbour cards: 0 pushed off to the sides, 1 attached.
    pub neighbours: f64,
    pub neighbours_target: f64,
    /// A lift paused (Quickstep's motion pause): the switcher on release.
    pub lift_paused: bool,
    /// Consecutive slow move samples toward a pause.
    pub lift_slow: u32,
    /// The released lift settling into its card or back (`lift.blend`).
    pub lift_settle: Option<crate::launcher_motion::Tween>,
    /// `overview` on a timed ease (a Recents card opening).
    pub overview_tween: Option<crate::launcher_motion::Tween>,
    /// The All Apps panel settling (pager pages).
    pub drawer_tween: Option<crate::launcher_motion::Tween>,
    /// Haptics asked for since the host last took them.
    pub haptics: Vec<crate::launcher_motion::Haptic>,
    /// The Recents page last ticked for.
    pub card_tick_page: i64,
}
/// See [`PhoneState::lift`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lift {
    pub rect: Rect,
    pub blend: f64,
    pub blend_v: f64,
}
/// Where a bottom swipe holds the open app (Pixel Launcher's swipe-up,
/// Quickstep `SwipeUpAnimationLogic` + `AnimatorControllerWithResistance`):
/// its BOTTOM edge stays under the finger — `dy_up` points up and `dx`
/// sideways, gain 1 in screen pixels. Over the travel `L` from the app's
/// bottom to its Recents card's bottom the window shrinks LINEARLY to the
/// card's scale (`card_scale`), so its bottom reaches the card's exactly
/// when the finger does; past that it keeps shrinking, easing (decelerate)
/// toward half size by the time the finger reaches the top of the screen.
pub fn lift_rect(app: Rect, card_scale: f64, dy_up: f64, dx: f64, screen_height: f64) -> Rect {
    let d = dy_up.max(0.0);
    let c = card_scale.clamp(0.3, 1.0);
    let tracking = (app.size.y * (1.0 - c) * 0.5 + 4.0).max(1.0);
    let scale = if d <= tracking {
        1.0 + (c - 1.0) * d / tracking
    } else {
        // The travel left until the finger is at the top of the screen.
        let rest = (screen_height.min(app.pos.y + app.size.y) - tracking).max(1.0);
        let t = ((d - tracking) / rest).clamp(0.0, 1.0);
        let decel = 1.0 - (1.0 - t) * (1.0 - t);
        c + (LIFT_FLOOR - c).min(0.0) * decel
    };
    let bottom = app.pos.y + app.size.y - d;
    let centre_x = app.pos.x + app.size.x * 0.5 + dx;
    let size = app.size * scale;
    Rect { pos: dvec2(centre_x - size.x * 0.5, bottom - size.y), size }
}
/// The smallest the lifted app gets, finger at the top of the screen
/// (Quickstep's resistance floor).
pub const LIFT_FLOOR: f64 = 0.5;
/// The Recents card's share of the app rect in this orientation.
pub fn card_scale(screen: Rect) -> f64 {
    if screen.size.x > screen.size.y { 0.74 } else { 0.76 }
}
impl Default for PhoneState {
    fn default() -> Self {
        Self { clock: "9:41".into(), wallpaper_time: 0.0, wallpaper_phase: 0.0, wallpaper_speed: 0.0, touch_time_offset: 0.0, frame_clock_delta: 0.0, screen: PhoneScreen::Home, client: None, order: Vec::new(),
            openness: 0.0, overview: 0.0, openness_v: 0.0, overview_v: 0.0, launch_from: None, origin: None, cards: Paging::default(), pager: Paging::default(), home_pages: 1, drawer: 0.0,
            home: HomeOrder::default(), home_order_loaded: false, edit: HomeEdit::default(), last_interaction: 0.0, dismiss_y: 0.0, gesture: None, touch: None,
            keyboard: 0.0, keyboard_target: 0.0, keyboard_sent_height: 0.0, keyboard_client: None,
            search_query: String::new(), search_focused: false, search_scroll: 0.0,
            ime: HashMap::new(), shift: false, symbols: false,
            desktop_size: None, desktop_clients: Vec::new(), desktop_style: DesktopStyle::Omarchy, viewport: Rect::default(),
            chrome: PhoneChrome::default(), band_from_app: false,
            tiles: HomeTiles::default(), lift: None,
            android: false, density: 1.0, open_elapsed: None, flight: None, reveal_elapsed: None,
            neighbours: 1.0, neighbours_target: 1.0, lift_paused: false, lift_slow: 0, lift_settle: None,
            overview_tween: None, drawer_tween: None, haptics: Vec::new(), card_tick_page: 0 }
    }
}
impl PhoneState {
    /// The home page (or the app library) is fully shown and nothing is
    /// animating or being dragged: safe to reconfigure a window down to
    /// its tile face without disturbing a closing animation.
    pub fn home_settled(&self) -> bool {
        matches!(self.screen, PhoneScreen::Home | PhoneScreen::Drawer)
            && self.gesture.is_none()
            && self.openness < 0.001
            && self.overview < 0.001
    }
    /// The client the person is looking at full screen (the open app, from
    /// the first frame of its zoom-in until it is dismissed).
    pub fn foreground(&self) -> Option<ClientId> {
        if self.screen == PhoneScreen::App { self.client } else { None }
    }
    /// The home page is on screen at all (tiles need drawing and driving).
    pub fn home_visible(&self) -> bool {
        self.screen != PhoneScreen::App || self.openness < 0.999 || (self.android && self.overview > 0.001)
    }
    pub fn activate(&mut self, client: ClientId) {
        self.search_focused = false;
        if self.client != Some(client) { self.keyboard_target = 0.0; self.lift = None; }
        self.client = Some(client);
        self.order.retain(|c| *c != client);
        self.order.insert(0, client);
        self.cards.set(0.0);
        self.leave_edit();
        // Opened from the drawer, the drawer stays behind the zoom until the
        // app covers it (`step` puts the pages back home then, unseen).
        if let Some((rect, in_drawer)) = self.launch_from.take() {
            self.origin = Some((client, rect, in_drawer));
        } else if self.origin.is_some_and(|(c, _, _)| c != client) {
            self.origin = None;
        }
        // Either way the finger that tapped no longer drags the pages.
        let page = if self.screen == PhoneScreen::Drawer { self.pager.target() } else { self.pager.target().min(self.last_home_page()) };
        self.pager.set_target(page, self.page_count());
        // Picked from the switcher: the app grows out of its card, only
        // `overview` easing down. From a switcher entered on Home its
        // openness was 0, and the two springs together pulled the card
        // toward the (small) icon before it grew — a shrink, then the
        // scale-up.
        if self.screen == PhoneScreen::Recents {
            self.openness = 1.0;
            self.openness_v = 0.0;
            self.overview_v = self.overview_v.min(0.0);
            // Android: the card grows to full screen in 336 ms on
            // TOUCH_RESPONSE (Quickstep `RECENTS_LAUNCH_DURATION`).
            if self.android {
                self.overview_tween = Some(crate::launcher_motion::Tween::new(self.overview, 0.0, crate::launcher_motion::RECENTS_LAUNCH_SECS, crate::launcher_motion::Curve::TouchResponse));
            }
        } else if self.android && self.openness < 0.5 && self.flight.is_none() {
            // Android: opened from its icon or tile — 500 ms, emphasized
            // curves, out of a circle the icon's size.
            self.open_elapsed = Some(0.0);
            self.openness = 0.0;
            self.openness_v = 0.0;
        }
        self.flight = None;
        self.reveal_elapsed = None;
        self.screen = PhoneScreen::App;
        self.dismiss_y = 0.0;
    }
    pub fn navigate(&mut self, screen: PhoneScreen) {
        self.search_focused = false;
        if screen == PhoneScreen::Recents {
            // The switcher shows every card: a paused lift slides them in
            // (`settle_lift`), anything else has them there at once.
            if self.lift_settle.is_none() {
                self.neighbours = 1.0;
            }
            self.neighbours_target = 1.0;
        }
        self.screen = screen;
        self.keyboard_target = 0.0;
        self.gesture = None;
        self.dismiss_y = 0.0;
        if screen != PhoneScreen::Home { self.leave_edit(); }
        let count = self.page_count();
        let page = if screen == PhoneScreen::Drawer { self.library_page() } else { self.pager.target().round().min(self.last_home_page()) };
        self.pager.set_target(page, count);
        // Android: All Apps opens in 600 ms and closes in 300 ms when not
        // dragged (Launcher `config_allAppsOpenDuration`/`CloseDuration`).
        self.drawer_tween = None;
        if self.android && (screen == PhoneScreen::Drawer || self.drawer > 0.001) && page != self.pager.page {
            let secs = if screen == PhoneScreen::Drawer { 0.6 } else { 0.3 };
            self.drawer_tween = Some(crate::launcher_motion::Tween::new(self.pager.page, page, secs, crate::launcher_motion::Curve::Decelerate));
            if screen == PhoneScreen::Drawer {
                self.haptics.push(crate::launcher_motion::Haptic::VirtualKey);
            }
        }
    }
    /// The pager's pages: the home page(s), then the library.
    pub fn page_count(&self) -> usize {
        self.home_pages.max(1) + 1
    }
    pub fn library_page(&self) -> f64 {
        self.home_pages.max(1) as f64
    }
    pub fn last_home_page(&self) -> f64 {
        self.library_page() - 1.0
    }
    /// The finger lands on the home page or the library: the pager holds
    /// its page until the finger lifts.
    pub fn library_drag_begin(&mut self) {
        // A settling panel stops under the finger.
        self.drawer_tween = None;
        self.pager.drag_begin();
    }
    /// The finger moved `d` points along the pan (right positive on iOS,
    /// where the pages pan sideways; down positive on Android, whose drawer
    /// is a sheet), over pages `extent` points wide: the pages follow 1:1,
    /// the ends rubber-band, `drawer` follows the pager.
    pub fn library_drag_move(&mut self, d: f64, extent: f64) {
        let count = self.page_count();
        self.pager.drag_move(d, extent, count);
        self.drawer = self.pager.page - self.last_home_page();
    }
    /// The finger lifts at `v` points per second along the pan: a flick
    /// flips one page its way, anything slower settles on the nearest;
    /// the screen follows the page it settles on. Eased from here.
    pub fn library_release(&mut self, v: f64, extent: f64, flick_speed: f64) {
        let count = self.page_count();
        self.pager.drag_end(v, extent, count, flick_speed);
        let screen = if self.pager.target() >= self.library_page() - 0.5 { PhoneScreen::Drawer } else { PhoneScreen::Home };
        self.search_focused = false;
        self.screen = screen;
        self.keyboard_target = 0.0;
        self.dismiss_y = 0.0;
        if screen != PhoneScreen::Home { self.leave_edit(); }
    }
    /// Whatever the finger was doing is taken away — the phone rotated, the
    /// style changed, the window lost focus: no release action runs, every
    /// half-dragged surface settles from rest where it is nearest, a carried
    /// icon stays in the slot it holds. True when a carried icon was set
    /// down (its order wants saving).
    pub fn cancel_gesture(&mut self) -> bool {
        let gesture = self.gesture.take();
        self.touch = None;
        self.dismiss_y = 0.0;
        self.cards.cancel_drag(self.order.len());
        if self.pager.dragging() {
            self.pager.cancel_drag(self.page_count());
            if matches!(self.screen, PhoneScreen::Home | PhoneScreen::Drawer) {
                self.screen = if self.pager.target() >= self.library_page() - 0.5 { PhoneScreen::Drawer } else { PhoneScreen::Home };
            }
        }
        if gesture.is_some() { self.openness_v = 0.0; self.overview_v = 0.0; }
        self.edit.drag.take().is_some()
    }
    /// Edit mode opens (a long press on an icon): the jiggle clock starts
    /// at `now`.
    pub fn enter_edit(&mut self, now: f64) {
        if !self.edit.active {
            self.edit = HomeEdit { active: true, since: now, drag: None, lift: 0.0 };
        }
    }
    pub fn leave_edit(&mut self) {
        self.edit.active = false;
        self.edit.drag = None;
    }
    /// The held icon follows the finger; the slot under the finger becomes
    /// its own, the others shifting aside. True when the order changed.
    pub fn drag_icon_to(&mut self, p: Vec2d, slot: Option<Slot>) -> bool {
        let Some(drag) = self.edit.drag.as_mut() else { return false };
        drag.pos = p;
        let Some(slot) = slot else { return false };
        if slot == drag.slot { return false; }
        let from = drag.slot;
        if self.home.move_to(from, slot) {
            if let Some(drag) = self.edit.drag.as_mut() { drag.slot = slot; }
            true
        } else {
            false
        }
    }
    pub fn step(&mut self, dt: f64) -> bool {
        let t = 1.0 - (-dt * 19.0).exp();
        // The app covers the screen: whatever it opened over (the drawer)
        // goes back to the home page behind it, where Home will find it.
        if self.screen == PhoneScreen::App && self.openness >= 0.999 && self.pager.target() > self.last_home_page() {
            let home = self.last_home_page();
            self.pager.set(home);
        }
        // Recents keeps whatever openness it was entered with: 1 from an
        // app (its card is the app pulled in, and tapping it opens it
        // back up), 0 from Home (cards only — nothing to pull, nothing to
        // wobble while the switcher fades in).
        let (open, overview) = self.spring_targets();
        let mut active = false;
        let dragging = self.gesture.as_ref().is_some_and(|g| g.bottom);
        // Android's timed and spring-driven motion (`launcher_motion`) runs
        // the values it owns; the generic springs leave those alone.
        let (timed_open, timed_overview) = self.step_android(dt, dragging, &mut active);
        for (value, speed, target, timed) in [(&mut self.openness, &mut self.openness_v, open, timed_open), (&mut self.overview, &mut self.overview_v, overview, timed_overview)] {
            if timed {
                *speed = 0.0;
                continue;
            }
            if dragging {
                *speed = 0.0;
                continue;
            }
            if *value == target && *speed == 0.0 {
                continue;
            }
            (*value, *speed) = spring_step(*value, *speed, target, dt);
            if (*value - target).abs() < 0.001 && speed.abs() < 0.01 {
                *value = target;
                *speed = 0.0;
            }
            active |= *value != target;
        }
        if let Some(lift) = self.lift.as_mut().filter(|_| self.lift_settle.is_none()) {
            if !dragging {
                (lift.blend, lift.blend_v) = spring_step(lift.blend, lift.blend_v, 0.0, dt);
                if lift.blend.abs() < 0.001 && lift.blend_v.abs() < 0.01 {
                    self.lift = None;
                } else {
                    active = true;
                }
            }
        }
        self.keyboard += (self.keyboard_target - self.keyboard) * t;
        if (self.keyboard_target - self.keyboard).abs() < 0.25 { self.keyboard = self.keyboard_target; }
        active |= self.keyboard != self.keyboard_target;
        active |= self.cards.step(dt);
        if let Some(mut tween) = self.drawer_tween.take() {
            let (page, going) = tween.step(dt);
            if going && !self.pager.dragging() {
                self.pager.set(page);
                self.drawer_tween = Some(tween);
                active = true;
            } else if !self.pager.dragging() {
                self.pager.set(tween.to);
            }
        } else {
            active |= self.pager.step(dt);
        }
        self.drawer = self.pager.page - self.last_home_page();
        // A Recents page crossed: a low tick.
        if self.android && self.screen == PhoneScreen::Recents {
            let page = self.cards.page.round() as i64;
            if page != self.card_tick_page {
                self.card_tick_page = page;
                self.haptics.push(crate::launcher_motion::Haptic::Tick);
            }
        }
        // Edit mode is an animation for as long as it lasts: the jiggle
        // asks for frames, the held icon's lift eases up and back down.
        let lift = if self.edit.active && self.edit.drag.is_some() { 1.0 } else { 0.0 };
        self.edit.lift += (lift - self.edit.lift) * (1.0 - (-dt * 18.0).exp());
        if (self.edit.lift - lift).abs() < 0.002 { self.edit.lift = lift; }
        active |= self.edit.active || self.edit.lift != lift;
        active
    }
    /// Android's motion for one frame (see the fields above): which of
    /// `openness` / `overview` it drove this frame.
    fn step_android(&mut self, dt: f64, dragging: bool, active: &mut bool) -> (bool, bool) {
        use crate::launcher_motion as lm;
        let mut timed_open = false;
        let mut timed_overview = false;
        if let Some(e) = self.open_elapsed.as_mut() {
            *e += dt;
            self.openness = (*e / lm::APP_OPEN_SECS).min(1.0);
            timed_open = true;
            *active = true;
            if *e >= lm::APP_OPEN_SECS.max(lm::LAUNCH_HOME_SECS) {
                self.open_elapsed = None;
            }
        }
        if let Some(flight) = self.flight.as_mut() {
            let going = flight.step(dt);
            self.openness = (1.0 - flight.progress.x).clamp(0.0, 1.0);
            timed_open = true;
            *active = true;
            if !going {
                self.flight = None;
                self.openness = 0.0;
            }
        }
        if let Some(e) = self.reveal_elapsed.as_mut() {
            *e += dt;
            *active = true;
            if *e >= lm::REVEAL_SECS {
                self.reveal_elapsed = None;
            }
        }
        if let Some(mut tween) = self.overview_tween.take() {
            let (v, going) = tween.step(dt);
            self.overview = v.clamp(0.0, 1.0);
            timed_overview = true;
            *active = true;
            if going { self.overview_tween = Some(tween); }
        }
        if !dragging {
            if let Some(mut tween) = self.lift_settle.take() {
                let (blend, going) = tween.step(dt);
                *active = true;
                match self.lift.as_mut() {
                    Some(lift) if going => {
                        lift.blend = blend;
                        self.lift_settle = Some(tween);
                    }
                    _ => self.lift = None,
                }
            }
        }
        if self.neighbours != self.neighbours_target {
            let step = dt / lm::RECENTS_ATTACH_SECS;
            self.neighbours = if self.neighbours < self.neighbours_target {
                (self.neighbours + step).min(self.neighbours_target)
            } else {
                (self.neighbours - step).max(self.neighbours_target)
            };
            *active = true;
        }
        (timed_open, timed_overview)
    }
    /// The home screen's scale and alpha for this frame (Android): All Apps
    /// pushing it back and hiding it at 40 %, a launch pushing it to 0.97,
    /// the reveal after a swipe home bringing it from 0.85.
    pub fn home_look(&self) -> (f64, f64) {
        use crate::launcher_motion as lm;
        if !self.android {
            return (1.0, 1.0);
        }
        let (mut scale, mut alpha, _, _, _) = lm::all_apps_frame(self.drawer.max(0.0));
        if let Some(e) = self.open_elapsed {
            scale *= lm::launch_home_scale(e);
        } else if self.screen == PhoneScreen::App && self.openness >= 0.999 {
            scale *= lm::LAUNCH_HOME_SCALE;
        }
        if let Some(e) = self.reveal_elapsed {
            let (s, a) = lm::reveal(e);
            scale *= s;
            alpha *= a;
        }
        // The switcher: the home screen recedes toward the launcher's hint
        // scale (0.92) and fades out, continuously with `overview`; the
        // wallpaper stays and dims (`recents_dim`).
        let o = self.overview.clamp(0.0, 1.0);
        scale *= 1.0 + (lm::OVERVIEW_HOME_SCALE - 1.0) * o;
        alpha *= 1.0 - o;
        (scale, alpha)
    }
    /// A lifted app released to Home (Android): it flies into `to` (its
    /// icon, or the dock-row circle), the home reveals behind it.
    pub fn fly_home(&mut self, to: Rect, to_radius: f64, velocity: Vec2d) {
        let Some(lift) = self.lift.take() else { return };
        let from_radius = crate::mobile_tiles::HomeMetrics::of(DesktopStyle::Android).card_radius;
        self.flight = Some(crate::launcher_motion::HomeFlight::new(lift.rect, from_radius, to, to_radius, velocity, self.density));
        self.reveal_elapsed = Some(0.0);
        self.lift_settle = None;
        self.openness = 1.0;
        self.openness_v = 0.0;
        self.overview = 0.0;
        self.overview_v = 0.0;
        self.neighbours = 0.0;
        self.neighbours_target = 0.0;
    }
    /// A released lift settles into its Recents card (overshoot) or back
    /// into the app (decelerate), in Quickstep's 350 ms.
    pub fn settle_lift(&mut self, to_recents: bool) {
        use crate::launcher_motion as lm;
        let Some(lift) = self.lift.as_ref() else { return };
        let (duration, curve) = if to_recents {
            (lm::swipe_settle_secs(1.0 - self.overview.min(1.0)).max(0.2), lm::Curve::Overshoot)
        } else {
            (lm::swipe_settle_secs(self.overview.max(0.3)), lm::Curve::Decelerate)
        };
        self.lift_settle = Some(lm::Tween::new(lift.blend, 0.0, duration, curve));
        // The switcher's backdrop eases with the card, never a jump.
        self.overview_tween = Some(lm::Tween::new(self.overview, if to_recents { 1.0 } else { 0.0 }, duration, lm::Curve::Decelerate));
        self.overview_v = 0.0;
        if to_recents {
            self.neighbours_target = 1.0;
        }
    }
    /// The All Apps panel released (Android, Launcher's rules): a fling
    /// faster than 1 dp/ms goes its way, else it opens past 40 % from Home
    /// or closes under 60 % from All Apps; it settles on Launcher's
    /// duration and curve. `v` points/s, up positive.
    pub fn all_apps_release(&mut self, v_up: f64, from_drawer: bool) {
        use crate::launcher_motion as lm;
        let count = self.page_count();
        self.pager.cancel_drag(count);
        let p = self.drawer;
        let open = if v_up.abs() > lm::ALL_APPS_FLING {
            v_up > 0.0
        } else if from_drawer {
            p >= lm::ALL_APPS_CLOSE_AT
        } else {
            p > lm::ALL_APPS_OPEN_AT
        };
        let target = if open { self.library_page() } else { self.last_home_page() };
        let (secs, fast) = lm::all_apps_settle(target - self.pager.page, v_up, self.density);
        self.drawer_tween = Some(lm::Tween::new(self.pager.page, target, secs, if fast { lm::Curve::Scroll } else { lm::Curve::ScrollCubic }));
        if open && !from_drawer {
            self.haptics.push(lm::Haptic::VirtualKey);
        }
        self.search_focused = false;
        self.keyboard_target = 0.0;
        self.dismiss_y = 0.0;
        self.screen = if open { PhoneScreen::Drawer } else { PhoneScreen::Home };
        if self.screen != PhoneScreen::Home { self.leave_edit(); }
    }
    /// A bottom drag in flight, `dy` points up the screen (negative). From
    /// a full-screen app the app pulls into its card (`openness` stays 1,
    /// `overview` follows the finger); from Home or the library only the
    /// switcher fades in with the finger — nothing is pulled, `openness`
    /// stays 0 — so a release into Recents has its cards already there.
    pub fn bottom_drag(&mut self, from: PhoneScreen, dy: f64, screen_height: f64) {
        self.overview = (-dy / (screen_height * 0.42)).clamp(0.0, 1.0);
        if from == PhoneScreen::App {
            self.openness = 1.0;
        }
    }
    /// The open app follows a bottom swipe: `delta` is the finger's travel
    /// since it went down (points; up is negative y). See [`lift_rect`].
    pub fn lift_follow(&mut self, delta: Vec2d, screen: Rect) {
        let app = app_rect(screen, self.chrome);
        let rect = lift_rect(app, card_scale(screen), -delta.y, delta.x, screen.size.y);
        self.lift = Some(Lift { rect, blend: 1.0, blend_v: 0.0 });
    }
    /// Where `openness` and `overview` are heading: 1/0 for the open app,
    /// the switcher keeps the openness it was entered with (1 from an app,
    /// 0 from Home).
    pub fn spring_targets(&self) -> (f64, f64) {
        let open = match self.screen {
            PhoneScreen::App if self.client.is_some() => 1.0,
            PhoneScreen::Recents if self.client.is_some() && self.openness > 0.5 => 1.0,
            _ => 0.0,
        };
        (open, if self.screen == PhoneScreen::Recents { 1.0 } else { 0.0 })
    }
    pub fn accepts_app_input(&self) -> bool {
        self.screen == PhoneScreen::App && self.gesture.is_none()
            && self.overview < 0.01 && self.openness > 0.99
    }
    pub fn keyboard_height(&self) -> f64 {
        if self.viewport.size.x > self.viewport.size.y { 184.0 } else { 292.0 }
    }
    pub fn searching(&self) -> bool {
        self.screen == PhoneScreen::Drawer && (self.search_focused || !self.search_query.is_empty())
    }
    /// The phone still needs frames at `now`: something eases, a finger is
    /// down, edit mode jiggles, or the interaction tail has not run out.
    /// After that the phone rests and paints nothing until touched.
    /// Advance the wallpaper by `dt`: full speed while `active` (a finger, an
    /// ease, the interaction tail), then its speed decays over a few seconds
    /// to rest. True while it still moves.
    pub fn step_wallpaper(&mut self, dt: f64, active: bool) -> bool {
        if active {
            self.wallpaper_speed = (self.wallpaper_speed + dt / WALLPAPER_RAMP_SECS).min(1.0);
        } else {
            self.wallpaper_speed *= (-dt / WALLPAPER_SETTLE_SECS).exp();
            if self.wallpaper_speed < 0.01 {
                self.wallpaper_speed = 0.0;
            }
        }
        self.wallpaper_phase += dt * self.wallpaper_speed;
        self.wallpaper_speed > 0.0
    }

    pub fn wants_frames(&self, now: f64, moving: bool) -> bool {
        moving || self.gesture.is_some() || self.edit.active || now - self.last_interaction < INTERACTION_TAIL_SECS
    }
}

pub fn phone_size(style: DesktopStyle) -> Vec2d {
    if style == DesktopStyle::Ios { dvec2(402.0, 874.0) } else { dvec2(412.0, 892.0) }
}
/// The viewport an open app gets: the screen minus what the chrome
/// reserves above (status bar) and below (home indicator).
pub fn app_rect(screen: Rect, chrome: PhoneChrome) -> Rect {
    let top = chrome.top_reserve(screen);
    let bottom = chrome.bottom_reserve(screen);
    Rect { pos: screen.pos + dvec2(0.0, top), size: dvec2(screen.size.x, (screen.size.y - top - bottom).max(1.0)) }
}
/// How far the Android drawer sheet travels, and so how far the finger
/// drags it: from the bottom of the app area (just above the home
/// indicator) to the top of the screen. One extent for the finger and the
/// sheet, so the sheet stays under the finger.
pub fn drawer_extent(screen: Rect, chrome: PhoneChrome) -> f64 {
    (screen.size.y - chrome.bottom_reserve(screen)).max(1.0)
}
pub fn card_rect(screen: Rect, chrome: PhoneChrome, index: f64, page: f64) -> Rect {
    card_rect_for(DesktopStyle::Ios, screen, chrome, index, page)
}
/// Card `index` in `style`'s Recents with `page` in the middle; cards sit
/// the skin's card gap apart (`HomeMetrics::card_gap`).
pub fn card_rect_for(style: DesktopStyle, screen: Rect, chrome: PhoneChrome, index: f64, page: f64) -> Rect {
    let app = app_rect(screen, chrome);
    let size = app.size * card_scale(screen);
    Rect { pos: app.pos + (app.size - size) * 0.5 + dvec2((index-page)*card_pitch(style, screen, chrome), -4.0), size }
}
/// From one Recents card to the next: a card's width and the gap.
pub fn card_pitch(style: DesktopStyle, screen: Rect, chrome: PhoneChrome) -> f64 {
    let scale = if screen.size.x > screen.size.y { 0.74 } else { 0.76 };
    app_rect(screen, chrome).size.x * scale + crate::mobile_tiles::HomeMetrics::of(style).card_gap
}
pub fn mix_rect(a: Rect, b: Rect, t: f64) -> Rect {
    Rect { pos: a.pos + (b.pos-a.pos)*t, size: a.size + (b.size-a.size)*t }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// A bottom swipe carries the open app with the finger: its bottom edge
    /// stays under the finger (within 2 pt) the whole drag while it shrinks
    /// toward the card — never the old gain of about 0.3 (the card mix
    /// alone), nor a centre-anchored card whose bottom outruns the finger.
    #[test]
    fn bottom_swipe_card_follows_the_finger() {
        for size in [dvec2(412.0, 892.0), dvec2(892.0, 412.0)] {
            let screen = Rect { pos: dvec2(0.0, 0.0), size };
            let mut phone = PhoneState::default();
            phone.chrome = PhoneChrome::Device { insets: SafeAreaInsets { top: 66.0, right: 0.0, bottom: 24.2, left: 0.0 } };
            let app = app_rect(screen, phone.chrome);
            let (bottom0, centre0) = (app.pos.y + app.size.y, app.pos.y + app.size.y * 0.5);
            let mut last_scale = 1.0;
            for step in 1..=40 {
                let dy = step as f64 * size.y * 0.02;
                phone.bottom_drag(PhoneScreen::App, -dy, size.y);
                phone.lift_follow(dvec2(0.0, -dy), screen);
                let r = phone.lift.unwrap().rect;
                let bottom = bottom0 - (r.pos.y + r.size.y);
                assert!((bottom - dy).abs() <= 2.0, "bottom moved {bottom} for a finger travel of {dy}");
                let _ = centre0;
                let scale = r.size.x / app.size.x;
                assert!(scale <= last_scale + 1e-9 && scale >= LIFT_FLOOR - 1e-9, "scale {scale}");
                // Linear to the card's scale over the tracking travel.
                let c = card_scale(screen);
                let tracking = app.size.y * (1.0 - c) * 0.5 + 4.0;
                if dy <= tracking {
                    assert!((scale - (1.0 + (c - 1.0) * dy / tracking)).abs() < 1e-6);
                }
                last_scale = scale;
            }
            // Released: the lift hands over to the springs without a jump.
            phone.gesture = None;
            let before = phone.lift.unwrap().rect;
            phone.step(1.0 / 120.0);
            let after = phone.lift.map(|l| l.blend).unwrap_or(0.0);
            assert!(after > 0.9 && after < 1.0, "blend {after} right after release");
            assert!(before.size.x > 0.0);
            for _ in 0..240 { phone.step(1.0 / 120.0); }
            assert!(phone.lift.is_none(), "the lift settles and lets go");
        }
    }
    #[test]
    fn both_orientations_reserve_system_bars_and_keep_selected_card_inside() {
        for size in [phone_size(DesktopStyle::Ios), phone_size(DesktopStyle::Android)] {
            for size in [size, dvec2(size.y, size.x)] {
                let screen = Rect { pos: dvec2(0.0, 32.0), size: size-dvec2(0.0,32.0) };
                let app = app_rect(screen, PhoneChrome::Simulated);
                let card = card_rect(screen, PhoneChrome::Simulated, 2.0, 2.0);
                assert!(app.size.x > 0.0 && app.size.y > 200.0);
                assert!(app.pos.y > screen.pos.y);
                assert!(card.pos.x >= app.pos.x && card.pos.y >= app.pos.y);
                assert!(card.pos.x+card.size.x <= app.pos.x+app.size.x);
                assert!(card.pos.y+card.size.y <= app.pos.y+app.size.y);
            }
        }
    }
    #[test]
    fn the_desktop_skin_fakes_the_phone_and_the_device_draws_none_of_it() {
        let portrait = Rect { pos: dvec2(0.0, 0.0), size: phone_size(DesktopStyle::Ios) };
        let landscape = Rect { pos: dvec2(0.0, 0.0), size: dvec2(portrait.size.y, portrait.size.x) };
        let phone = SafeAreaInsets { top: 59.0, right: 0.0, bottom: 34.0, left: 0.0 };
        // The desktop skin: every fake, the 42/24 reserve, insets ignored.
        let sim = PhoneChrome::for_host(false, phone);
        assert_eq!(sim, PhoneChrome::Simulated);
        assert_eq!(sim, PhoneChrome::default(), "before startup reads the host: the desktop");
        assert!(sim.fake_status() && sim.fake_indicator() && sim.controls_strip());
        let app = app_rect(portrait, sim);
        assert_eq!((app.pos.y, app.size.y), (42.0, portrait.size.y - 42.0 - 24.0));
        assert_eq!(app_rect(landscape, sim).pos.y, 24.0);
        let mut still = sim;
        still.set_insets(phone);
        assert_eq!(still, sim, "a simulated phone never takes insets");
        // The phone: nothing fake, the OS's own insets, kept current.
        let mut dev = PhoneChrome::for_host(true, phone);
        assert!(!dev.fake_status() && !dev.fake_indicator() && !dev.controls_strip());
        let app = app_rect(portrait, dev);
        assert_eq!((app.pos.y, app.size.y), (59.0, portrait.size.y - 59.0 - 34.0));
        let turned = SafeAreaInsets { top: 0.0, right: 59.0, bottom: 21.0, left: 59.0 };
        dev.set_insets(turned);
        let app = app_rect(landscape, dev);
        // The native home gesture strip keeps its 28-point minimum even
        // when the OS reports a smaller landscape inset.
        assert_eq!((app.pos.y, app.size.y), (0.0, landscape.size.y - 28.0));
        assert!(card_rect(landscape, dev, 0.0, 0.0).pos.y >= app.pos.y);
    }

    #[test]
    fn the_home_gesture_zone_is_the_bottom_reserve_on_both_phones() {
        let screen = Rect { pos: dvec2(0.0, 0.0), size: phone_size(DesktopStyle::Ios) };
        let bottom = screen.size.y;
        let at = |y: f64| dvec2(200.0, y);
        // The phone: a touch that starts AT the edge, or anywhere under the
        // real 34 pt home indicator (plus 4 of slack), is the shell's.
        let dev = PhoneChrome::for_host(true, SafeAreaInsets { top: 59.0, right: 0.0, bottom: 34.0, left: 0.0 });
        assert!(dev.bottom_zone(screen, at(bottom - 1.0)));
        assert!(dev.bottom_zone(screen, at(bottom - 37.0)));
        assert!(!dev.bottom_zone(screen, at(bottom - 40.0)));
        // The desktop skin: its fake 24 pt indicator strip, plus the same slack.
        let sim = PhoneChrome::Simulated;
        assert!(sim.bottom_zone(screen, at(bottom - 1.0)));
        assert!(sim.bottom_zone(screen, at(bottom - 27.0)));
        assert!(!sim.bottom_zone(screen, at(bottom - 30.0)));
        // The zone follows the window, not the origin.
        let shifted = Rect { pos: dvec2(80.0, 60.0), size: screen.size };
        assert!(dev.bottom_zone(shifted, dvec2(200.0, 60.0 + bottom - 1.0)));
        assert!(!dev.bottom_zone(shifted, dvec2(200.0, bottom - 1.0)));
    }

    #[test]
    fn a_bottom_swipe_flicks_home_holds_for_the_switcher_and_always_leaves_it() {
        let h = 874.0;
        let up = |y: f64| dvec2(0.0, y);
        // Too short to mean anything.
        assert_eq!(bottom_swipe_target(PhoneScreen::App, false, up(-30.0), h, 0.1, false, -900.0), None);
        // A real finger: 220 pt in 0.45 s, still moving at release — home.
        assert_eq!(bottom_swipe_target(PhoneScreen::App, false, up(-220.0), h, 0.45, false, -700.0), Some(PhoneHit::Home));
        // The same distance, but the finger rested before letting go — the switcher.
        assert_eq!(bottom_swipe_target(PhoneScreen::App, false, up(-220.0), h, 0.45, true, 0.0), Some(PhoneHit::Recents));
        // Released far up the screen without resting: home.
        assert_eq!(bottom_swipe_target(PhoneScreen::App, false, up(-400.0), h, 1.2, false, -80.0), Some(PhoneHit::Home));
        // The hold wins over the distance: rested before lifting, the switcher.
        assert_eq!(bottom_swipe_target(PhoneScreen::App, false, up(-400.0), h, 1.2, true, 0.0), Some(PhoneHit::Recents));
        // Short and quick (the bridge's injected swipe): home.
        assert_eq!(bottom_swipe_target(PhoneScreen::App, false, up(-120.0), h, 0.1, false, 0.0), Some(PhoneHit::Home));
        // Slow, short, not moving: the switcher.
        assert_eq!(bottom_swipe_target(PhoneScreen::App, false, up(-80.0), h, 0.6, false, -100.0), Some(PhoneHit::Recents));
        // From the switcher every upward swipe leaves it.
        assert_eq!(bottom_swipe_target(PhoneScreen::Recents, false, up(-60.0), h, 0.9, true, 0.0), Some(PhoneHit::Home));
        // Android's Home: the strip is multitasking, a flick included.
        assert_eq!(bottom_swipe_target(PhoneScreen::Home, true, up(-200.0), h, 0.1, false, -900.0), Some(PhoneHit::Recents));
        assert_eq!(bottom_swipe_target(PhoneScreen::Home, true, up(-300.0), h, 0.8, true, 0.0), Some(PhoneHit::Recents));
    }

    /// Android's Home by where the finger starts: in the bottom strip it is
    /// a bottom (multitasking) swipe, anywhere above it the drawer's.
    /// The wallpaper slows down to a standstill after the last interaction
    /// (no jump: its phase only advances by its own speed) and then asks
    /// for no frames; a touch brings it back up to speed.
    /// Switching to an app grows it monotonically from where it is (its
    /// card, or its icon) to full screen: never a shrink first.
    #[test]
    fn switching_to_an_app_only_grows() {
        let screen = Rect { pos: dvec2(0.0, 0.0), size: dvec2(412.0, 892.0) };
        let icon = Rect { pos: dvec2(40.0, 600.0), size: dvec2(60.0, 60.0) };
        for (entered_from_app, via_recents) in [(false, true), (true, true), (false, false)] {
            let mut phone = PhoneState::default();
            phone.order = vec![1 as ClientId, 2 as ClientId];
            if entered_from_app { phone.client = Some(1 as ClientId); phone.openness = 1.0; }
            if via_recents { phone.screen = PhoneScreen::Recents; phone.overview = 1.0; }
            let target = 2 as ClientId;
            phone.activate(target);
            let app = app_rect(screen, phone.chrome);
            let rect = |p: &PhoneState| {
                let index = p.order.iter().position(|c| *c == target).unwrap() as f64;
                let card = card_rect_for(DesktopStyle::Android, screen, p.chrome, index, p.cards.page);
                mix_rect(mix_rect(icon, app, p.openness), card, p.overview)
            };
            let mut last = rect(&phone).size.x;
            for _ in 0..240 {
                phone.step(1.0 / 120.0);
                let w = rect(&phone).size.x;
                assert!(w >= last - 0.01, "from_app={entered_from_app} recents={via_recents}: width {w} < {last}");
                last = w;
            }
            assert!((last - app.size.x).abs() < 0.5, "it ends full screen ({last})");
        }
    }

    /// Android's switcher: cards are there when it is opened from Home (the
    /// neighbours only wait for a paused lift), the backdrop eases with a
    /// settling lift instead of jumping, and the home screen behind it
    /// recedes continuously.
    #[test]
    fn android_switcher_cards_and_backdrop_are_continuous() {
        let mut phone = PhoneState::default();
        phone.android = true;
        phone.order = vec![1 as ClientId];
        phone.neighbours = 0.0;
        phone.neighbours_target = 0.0;
        phone.navigate(PhoneScreen::Recents);
        assert_eq!(phone.neighbours, 1.0, "from Home every card shows at once");

        let mut phone = PhoneState::default();
        phone.android = true;
        phone.screen = PhoneScreen::App;
        phone.client = Some(1 as ClientId);
        phone.openness = 1.0;
        phone.overview = 0.3;
        phone.lift = Some(Lift { rect: Rect::default(), blend: 1.0, blend_v: 0.0 });
        phone.settle_lift(true);
        phone.navigate(PhoneScreen::Recents);
        let mut last = phone.overview;
        assert!((last - 0.3).abs() < 1e-9, "no jump at release");
        for _ in 0..120 {
            phone.step(1.0 / 120.0);
            assert!(phone.overview >= last - 1e-9 && phone.overview - last < 0.2, "eases up");
            last = phone.overview;
        }
        assert!((phone.overview - 1.0).abs() < 1e-6);
        let (s, a) = phone.home_look();
        assert!(a < 1e-6 && s < 1.0, "the home has receded and faded behind the cards");
    }

    #[test]
    fn wallpaper_settles_then_rests() {
        let mut phone = PhoneState::default();
        let dt = 1.0 / 120.0;
        for _ in 0..240 { assert!(phone.step_wallpaper(dt, true)); }
        assert!((phone.wallpaper_speed - 1.0).abs() < 1e-9);
        let mut steps = 0;
        let mut last = phone.wallpaper_phase;
        let mut last_step = f64::MAX;
        while phone.step_wallpaper(dt, false) {
            let step = phone.wallpaper_phase - last;
            assert!(step <= last_step + 1e-12, "it only slows down");
            last_step = step;
            last = phone.wallpaper_phase;
            steps += 1;
            assert!(steps < 120 * 10, "it comes to rest");
        }
        assert!(steps > 120 * 3, "a slow settle, not a stop ({steps} frames)");
        let rest = phone.wallpaper_phase;
        assert!(!phone.step_wallpaper(dt, false));
        assert_eq!(phone.wallpaper_phase, rest, "at rest it does not move");
        assert!(phone.step_wallpaper(dt, true), "a touch starts it again");
        assert!(phone.wallpaper_phase - rest < dt, "without a jump");
    }

    #[test]
    fn android_home_swipe_start_zones() {
        let screen = Rect { pos: dvec2(0.0, 0.0), size: dvec2(412.0, 892.0) };
        let chrome = PhoneChrome::Device { insets: SafeAreaInsets { top: 42.0, right: 0.0, bottom: 24.0, left: 0.0 } };
        let strip_top = screen.size.y - chrome.bottom_reserve(screen) - 4.0;
        assert!(chrome.bottom_zone(screen, dvec2(206.0, screen.size.y - 6.0)));
        assert!(chrome.bottom_zone(screen, dvec2(206.0, strip_top + 1.0)));
        for y in [100.0, 450.0, strip_top - 20.0] {
            assert!(!chrome.bottom_zone(screen, dvec2(206.0, y)), "{y} is the drawer's");
        }
        let up = |dy| dvec2(0.0, dy);
        for (dy, dur, held, vy) in [(-120.0, 0.1, false, -1500.0), (-300.0, 0.6, false, -300.0), (-200.0, 0.8, true, 0.0)] {
            assert_eq!(bottom_swipe_target(PhoneScreen::Home, true, up(dy), 892.0, dur, held, vy), Some(PhoneHit::Recents));
        }
        assert_eq!(bottom_swipe_target(PhoneScreen::Home, true, up(-20.0), 892.0, 0.1, false, 0.0), None);
        // From an app: 36 dp up goes home, a pause the switcher, less drops back.
        assert_eq!(bottom_swipe_target(PhoneScreen::App, true, up(-40.0), 892.0, 0.5, false, -60.0), Some(PhoneHit::Home));
        assert_eq!(bottom_swipe_target(PhoneScreen::App, true, up(-300.0), 892.0, 0.9, true, 0.0), Some(PhoneHit::Recents));
        assert_eq!(bottom_swipe_target(PhoneScreen::App, true, up(-30.0), 892.0, 0.1, false, -900.0), None);
    }

    #[test]
    fn pages_follow_the_finger_flip_one_on_a_flick_and_settle() {
        let (width, count) = (300.0, 4);
        let mut cards = Paging::default();
        // The page follows the finger 1:1.
        cards.drag_begin();
        cards.drag_move(-150.0, width, count);
        assert!((cards.page - 0.5).abs() < 1e-9);
        // A slow release settles on the nearest page, eased, then rests.
        cards.drag_end(-50.0, width, count, CARD_FLICK_SPEED);
        let mut frames = 0;
        while cards.step(1.0 / 60.0) { frames += 1; assert!(frames < 200); }
        assert_eq!(cards.page, 1.0);
        assert!(cards.settled() && frames > 3, "one ease, not a snap: {frames} frames");
        assert!(!cards.step(1.0 / 60.0), "settled pages request no frames");
        // A short drag with a fast flick advances exactly one page.
        cards.drag_begin();
        cards.drag_move(-20.0, width, count);
        cards.drag_end(-2000.0, width, count, CARD_FLICK_SPEED);
        while cards.step(1.0 / 60.0) {}
        assert_eq!(cards.page, 2.0);
        // A flick back goes one page back, never several.
        cards.drag_begin();
        cards.drag_move(30.0, width, count);
        cards.drag_end(3000.0, width, count, CARD_FLICK_SPEED);
        while cards.step(1.0 / 60.0) {}
        assert_eq!(cards.page, 1.0);
        // The ends rubber-band and settle back.
        cards.set(0.0);
        cards.drag_begin();
        cards.drag_move(300.0, width, count);
        assert!(cards.page < 0.0 && cards.page > -0.5, "a third of the overshoot: {}", cards.page);
        cards.drag_end(0.0, width, count, CARD_FLICK_SPEED);
        while cards.step(1.0 / 60.0) {}
        assert_eq!(cards.page, 0.0);
        cards.set(3.0);
        cards.drag_begin();
        cards.drag_move(-600.0, width, count);
        assert!(cards.page > 3.0 && cards.page < 3.8);
        cards.drag_end(-5000.0, width, count, CARD_FLICK_SPEED);
        while cards.step(1.0 / 60.0) {}
        assert_eq!(cards.page, 3.0, "a flick past the last page stays on it");
    }

    #[test]
    fn the_library_pans_with_the_finger_and_settles_by_distance_or_flick() {
        let width = 402.0;
        let mut phone = PhoneState::default();
        assert_eq!((phone.page_count(), phone.library_page()), (2, 1.0));
        assert_eq!(phone.drawer, 0.0);
        // The finger drags the page left: the library follows 1:1.
        phone.library_drag_begin();
        phone.library_drag_move(-120.6, width);
        assert!((phone.drawer - 0.3).abs() < 1e-9 && phone.pager.dragging());
        // Held: stepping does not move it.
        phone.step(1.0 / 60.0);
        assert!((phone.drawer - 0.3).abs() < 1e-9);
        // Released short and slow: back home, eased.
        phone.library_release(-50.0, width, PAGE_FLICK_SPEED);
        assert_eq!(phone.screen, PhoneScreen::Home);
        let mut frames = 0;
        while phone.step(1.0 / 60.0) { frames += 1; assert!(frames < 200); }
        assert_eq!(phone.drawer, 0.0);
        assert!(frames > 3, "one ease, not a snap: {frames} frames");
        // A flick completes the open whatever the distance.
        phone.library_drag_begin();
        phone.library_drag_move(-30.0, width);
        phone.library_release(-1500.0, width, PAGE_FLICK_SPEED);
        assert_eq!(phone.screen, PhoneScreen::Drawer);
        while phone.step(1.0 / 60.0) {}
        assert_eq!(phone.drawer, 1.0);
        // Past the library only the rubber band's share; released, it settles back on it.
        phone.library_drag_begin();
        phone.library_drag_move(-200.0, width);
        assert!(phone.drawer > 1.0 && phone.drawer < 1.2, "{}", phone.drawer);
        phone.library_release(0.0, width, PAGE_FLICK_SPEED);
        while phone.step(1.0 / 60.0) {}
        assert_eq!((phone.screen, phone.drawer), (PhoneScreen::Drawer, 1.0));
        // Coming back: the same pan the other way, a flick right closes.
        phone.library_drag_begin();
        phone.library_drag_move(80.4, width);
        assert!((phone.drawer - 0.8).abs() < 1e-6);
        phone.library_release(1500.0, width, PAGE_FLICK_SPEED);
        assert_eq!(phone.screen, PhoneScreen::Home);
        while phone.step(1.0 / 60.0) {}
        assert_eq!(phone.drawer, 0.0);
        assert!(!phone.step(1.0 / 60.0), "settled: no frames");
        // A tap on the page dots goes there eased, and Home brings it back.
        phone.navigate(PhoneScreen::Drawer);
        assert!(phone.step(1.0 / 60.0));
        while phone.step(1.0 / 60.0) {}
        assert_eq!(phone.drawer, 1.0);
        phone.navigate(PhoneScreen::Home);
        while phone.step(1.0 / 60.0) {}
        assert_eq!(phone.drawer, 0.0);
    }

    #[test]
    fn a_long_press_is_half_a_second_without_moving() {
        let mut g = PhoneGesture::new(dvec2(100.0, 100.0), 10.0, Some(PhoneHit::App("sheets".into())), false, false, PhoneScreen::Home);
        assert!(!g.long_press_due(10.3), "too soon");
        g.track(dvec2(104.0, 103.0));
        assert!(g.long_press_due(10.5), "held within the slop");
        g.long_pressed = true;
        assert!(!g.long_press_due(11.0), "fires once");
        let mut g = PhoneGesture::new(dvec2(100.0, 100.0), 10.0, None, false, false, PhoneScreen::Home);
        g.track(dvec2(112.0, 100.0));
        g.track(dvec2(100.0, 100.0));
        assert!(!g.long_press_due(11.0), "a finger that wandered and came back is a drag, not a press");
    }

    #[test]
    fn edit_mode_jiggles_moves_icons_live_and_rests_when_it_ends() {
        let mut phone = PhoneState::default();
        phone.home = HomeOrder { icons: vec!["sheets".into(), "clock".into(), "route".into()], dock: vec!["files".into()] };
        for _ in 0..80 { phone.step(1.0 / 60.0); }
        assert!(!phone.step(1.0 / 60.0), "at rest before");
        phone.enter_edit(5.0);
        assert!(phone.edit.active);
        assert!(phone.step(1.0 / 60.0), "edit mode asks for frames");
        // Every icon rocks within ±2°, each on its own phase.
        let a = phone.edit.jiggle(0, 5.2);
        let b = phone.edit.jiggle(1, 5.2);
        assert!(a.abs() <= JIGGLE_DEGREES + 1e-9 && b.abs() <= JIGGLE_DEGREES + 1e-9);
        assert!((a - b).abs() > 0.01, "phases differ");
        // Sheets is picked up and carried over the last slot: the others shift aside live.
        phone.edit.drag = Some(IconDrag { id: "sheets".into(), slot: Slot::Icon(0), pos: dvec2(0.0, 0.0), grab: dvec2(0.0, 0.0) });
        assert!(phone.drag_icon_to(dvec2(300.0, 0.0), Some(Slot::Icon(2))));
        assert_eq!(phone.home.icons, ["clock", "route", "sheets"]);
        assert_eq!(phone.edit.drag.as_ref().unwrap().slot, Slot::Icon(2));
        assert!(!phone.drag_icon_to(dvec2(310.0, 0.0), Some(Slot::Icon(2))), "same slot: nothing to do");
        assert!(!phone.drag_icon_to(dvec2(310.0, 0.0), None), "off the grid: the icon just follows");
        assert_eq!(phone.edit.drag.as_ref().unwrap().pos, dvec2(310.0, 0.0));
        // Into the dock beside Files.
        assert!(phone.drag_icon_to(dvec2(0.0, 800.0), Some(Slot::Dock(1))));
        assert_eq!((phone.home.icons.len(), phone.home.dock.as_slice()), (2, &["files".to_string(), "sheets".to_string()][..]));
        // The lift eases up while held, back down after the drop.
        for _ in 0..30 { phone.step(1.0 / 60.0); }
        assert!(phone.edit.lift > 0.9);
        phone.edit.drag = None;
        for _ in 0..60 { phone.step(1.0 / 60.0); }
        assert_eq!(phone.edit.lift, 0.0);
        assert!(phone.step(1.0 / 60.0), "still jiggling until Done");
        // Done: the animation ends with the mode; nothing asks for frames.
        phone.leave_edit();
        assert!(!phone.edit.active);
        assert_eq!(phone.edit.jiggle(0, 9.0), 0.0);
        assert!(!phone.step(1.0 / 60.0), "at rest after");
        assert!(!phone.wants_frames(20.0, false));
        phone.last_interaction = 20.0;
        assert!(phone.wants_frames(20.5, false) && !phone.wants_frames(21.5, false), "the tail runs a second");
        // Navigating away also ends it.
        phone.enter_edit(30.0);
        phone.navigate(PhoneScreen::Drawer);
        assert!(!phone.edit.active);
    }

    #[test]
    fn a_bottom_drag_on_home_never_raises_openness() {
        let mut phone = PhoneState::default();
        phone.activate(4);
        phone.navigate(PhoneScreen::Home);
        for _ in 0..80 { phone.step(1.0 / 60.0); }
        assert_eq!(phone.openness, 0.0);
        // The switcher fades in with the finger; the last app is not pulled.
        phone.bottom_drag(PhoneScreen::Home, -120.0, 874.0);
        assert!(phone.overview > 0.3 && phone.overview < 0.4);
        assert_eq!(phone.openness, 0.0);
        // Released into Recents from Home: cards only, openness stays down.
        phone.navigate(PhoneScreen::Recents);
        for _ in 0..80 { phone.step(1.0 / 60.0); }
        assert_eq!(phone.overview, 1.0);
        assert_eq!(phone.openness, 0.0);
        // From a full-screen app the same drag pulls the app into its card.
        phone.activate(4);
        for _ in 0..80 { phone.step(1.0 / 60.0); }
        phone.bottom_drag(PhoneScreen::App, -120.0, 874.0);
        assert_eq!(phone.openness, 1.0);
        assert!(phone.overview > 0.3);
        phone.navigate(PhoneScreen::Recents);
        for _ in 0..80 { phone.step(1.0 / 60.0); }
        assert_eq!((phone.overview, phone.openness), (1.0, 1.0));
    }

    #[test]
    fn home_and_task_switcher_preserve_instances_and_settle() {
        let mut phone = PhoneState::default();
        phone.activate(10); phone.activate(11); phone.activate(10);
        assert_eq!(phone.order, [10,11]);
        for screen in [PhoneScreen::Recents, PhoneScreen::Home, PhoneScreen::App] {
            phone.navigate(screen);
            for _ in 0..80 { phone.step(1.0/60.0); }
            assert_eq!(phone.client, Some(10));
            assert_eq!(phone.order.len(), 2);
            assert!(!phone.step(1.0/60.0));
        }
        assert!(phone.accepts_app_input());
    }
    #[test]
    fn compact_faces_wait_for_the_home_page_to_settle() {
        let mut phone = PhoneState::default();
        assert!(phone.home_settled() && phone.foreground().is_none());
        phone.activate(4);
        for _ in 0..80 { phone.step(1.0/60.0); }
        assert_eq!(phone.foreground(), Some(4));
        assert!(!phone.home_settled() && !phone.home_visible());
        phone.navigate(PhoneScreen::Home);
        assert_eq!(phone.foreground(), None, "dismissed: no longer the app the person looks at");
        assert!(!phone.home_settled(), "the window is still animating into its icon");
        assert!(phone.home_visible());
        for _ in 0..80 { phone.step(1.0/60.0); }
        assert!(phone.home_settled());
        phone.navigate(PhoneScreen::Recents);
        for _ in 0..80 { phone.step(1.0/60.0); }
        assert!(!phone.home_settled(), "Recents keeps every card in its full face");
    }

    #[test]
    fn the_spring_settles_in_a_third_of_a_second_and_carries_the_release_speed() {
        // From rest one page away: arrives without overshoot, then rests.
        let mut p = Paging::default();
        p.set(0.0);
        p.drag_begin();
        p.drag_move(-1.0, 400.0, 3);
        p.drag_end(0.0, 400.0, 3, PAGE_FLICK_SPEED);
        assert_eq!(p.target(), 0.0, "a nudge settles back");
        let mut p = Paging::default();
        p.set_target(1.0, 3);
        let mut t = 0.0;
        while p.step(1.0 / 120.0) {
            t += 1.0 / 120.0;
            assert!(p.page <= 1.0 + 1e-9, "critically damped: no overshoot");
            assert!(t < 0.5);
        }
        assert!(t > 0.2 && t < 0.4, "about 0.3 s: {t}");
        // A flick's speed carries into the first frame; a slow release eases from rest.
        let first = |v: f64| {
            let mut p = Paging::default();
            p.drag_begin();
            p.drag_move(-100.0, 400.0, 3);
            let at = p.page;
            p.drag_end(v, 400.0, 3, PAGE_FLICK_SPEED);
            p.step(1.0 / 60.0);
            p.page - at
        };
        assert!(first(-2000.0) > first(-10.0) * 2.0);
        assert!(first(-2000.0) > 0.05, "a flick moves the page at once: {}", first(-2000.0));
    }

    #[test]
    fn the_release_speed_is_the_last_80_ms_and_zero_after_a_rest() {
        let mut g = PhoneGesture::new(dvec2(0.0, 800.0), 0.0, None, true, false, PhoneScreen::App);
        // Slow start, fast finish: only the recent stretch counts.
        for i in 1..=10 { let t = i as f64 * 0.016; g.sample(dvec2(0.0, 800.0 - i as f64 * 2.0), t); g.last = dvec2(0.0, 800.0 - i as f64 * 2.0); }
        for i in 1..=6 { let t = 0.16 + i as f64 * 0.016; let y = 780.0 - i as f64 * 30.0; g.sample(dvec2(0.0, y), t); g.last = dvec2(0.0, y); }
        let v = g.release_velocity(0.26);
        assert!(v.y < -1500.0 && v.y > -2200.0, "{}", v.y);
        assert_eq!(g.release_velocity(0.26 + VELOCITY_REST + 0.01), dvec2(0.0, 0.0), "rested before lifting");
    }

    #[test]
    fn a_drag_takes_one_axis_past_the_slop() {
        let mut g = PhoneGesture::new(dvec2(0.0, 0.0), 0.0, None, false, false, PhoneScreen::Home);
        assert_eq!(g.lock_axis(dvec2(5.0, 1.0), true), None, "inside the slop");
        assert_eq!(g.lock_axis(dvec2(10.0, 9.0), true), None, "no axis leads yet");
        assert_eq!(g.lock_axis(dvec2(20.0, 9.0), true), Some(true));
        assert_eq!(g.lock_axis(dvec2(0.0, 90.0), true), Some(true), "decided once");
        let mut g = PhoneGesture::new(dvec2(0.0, 0.0), 0.0, None, false, false, PhoneScreen::Home);
        assert_eq!(g.lock_axis(dvec2(2.0, -30.0), true), Some(false));
    }

    #[test]
    fn the_drawer_sheet_travels_with_the_finger() {
        let screen = Rect { pos: dvec2(0.0, 32.0), size: dvec2(412.0, 860.0) };
        let extent = drawer_extent(screen, PhoneChrome::Simulated);
        let mut phone = PhoneState::default();
        phone.library_drag_begin();
        phone.library_drag_move(-200.0, extent);
        // The sheet's top rises (1 - drawer) * extent from the bottom: 200 pt.
        assert!(((1.0 - phone.drawer) * extent - (extent - 200.0)).abs() < 1e-6);
        // Slow and short: back down; the same distance flicked: open.
        phone.library_release(-100.0, extent, DRAWER_FLICK_SPEED);
        assert_eq!(phone.screen, PhoneScreen::Home);
        phone.library_drag_begin();
        phone.library_drag_move(-150.0, extent);
        phone.library_release(-1500.0, extent, DRAWER_FLICK_SPEED);
        assert_eq!(phone.screen, PhoneScreen::Drawer);
    }

    #[test]
    fn a_pause_breaks_the_path_and_a_twitch_does_not_undo_a_hold() {
        // Codex's case: fast to 32 ms, a hold, release with 1 pt of jitter at 240 ms.
        let mut g = PhoneGesture::new(dvec2(0.0, 800.0), 0.0, None, true, false, PhoneScreen::App);
        for (t, y) in [(0.016, 768.0), (0.032, 736.0)] { g.sample(dvec2(0.0, y), t); g.last = dvec2(0.0, y); }
        g.sample(dvec2(0.0, 735.0), 0.240);
        assert_eq!(g.release_velocity(0.240), dvec2(0.0, 0.0), "no stale flick across the pause");
        assert!(0.240 - g.last_time > SWIPE_HOLD_SECS, "the twitch kept the hold");
        // The window's start is interpolated: a steady 1000 pt/s reads 1000.
        let mut g = PhoneGesture::new(dvec2(0.0, 0.0), 0.0, None, false, false, PhoneScreen::Home);
        for i in 1..=20 { let t = i as f64 * 0.013; g.sample(dvec2(t * 1000.0, 0.0), t); }
        assert!((g.vx - 1000.0).abs() < 1.0, "{}", g.vx);
    }

    #[test]
    fn a_finger_past_the_slop_is_a_drag_to_the_end() {
        let mut g = PhoneGesture::new(dvec2(100.0, 100.0), 0.0, None, false, false, PhoneScreen::Home);
        g.track(dvec2(104.0, 100.0));
        assert!(!g.committed);
        g.track(dvec2(109.5, 100.0));
        g.track(dvec2(100.0, 100.0));
        assert!(g.committed, "came back to where it started, still a drag");
    }

    #[test]
    fn a_release_on_the_page_with_speed_still_moves_and_rests_only_when_slow() {
        let mut p = Paging::default();
        p.set(1.0);
        p.drag_begin();
        p.drag_move(0.0, 400.0, 3);
        p.drag_end(-300.0, 400.0, 3, PAGE_FLICK_SPEED);
        assert_eq!(p.target(), 1.0);
        assert!(!p.settled(), "on the page but still moving");
        let before = p.page;
        assert!(p.step(1.0 / 60.0), "a moving page asks for frames");
        assert!((p.page - before).abs() * 400.0 > 1.0, "the release speed carried: {}", (p.page - before) * 400.0);
        while p.step(1.0 / 60.0) {}
        assert!(p.settled() && p.page == 1.0);
    }

    #[test]
    fn a_gesture_taken_away_settles_from_rest_and_runs_no_release() {
        let mut phone = PhoneState::default();
        phone.gesture = Some(PhoneGesture::new(dvec2(0.0, 800.0), 0.0, None, false, false, PhoneScreen::Home));
        phone.library_drag_begin();
        phone.library_drag_move(-600.0, 868.0);
        assert!(phone.drawer > 0.5);
        phone.cards.drag_begin();
        phone.cards.drag_move(-100.0, 300.0, 3);
        assert!(!phone.cancel_gesture(), "no icon was carried");
        assert!(phone.gesture.is_none() && !phone.pager.dragging() && !phone.cards.dragging());
        assert_eq!(phone.screen, PhoneScreen::Drawer, "nearest: the drawer");
        while phone.step(1.0 / 60.0) {}
        assert_eq!((phone.drawer, phone.cards.page), (1.0, 0.0));
        assert!(!phone.wants_frames(100.0, false), "and the phone rests");
        phone.edit.drag = Some(IconDrag { id: "sheets".into(), slot: Slot::Icon(0), pos: dvec2(0.0, 0.0), grab: dvec2(0.0, 0.0) });
        assert!(phone.cancel_gesture(), "a carried icon is set down in its slot");
    }
}
