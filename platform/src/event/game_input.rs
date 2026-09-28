use crate::makepad_live_id::*;
use crate::makepad_math::Vec2;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum GameInputConnectedEvent {
    Connected(GameInputInfo),
    Disconnected(GameInputInfo),
}

#[derive(Clone, Debug, PartialEq)]
pub struct GameInputInfo {
    pub id: LiveId,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GameInputState {
    Gamepad(GamepadState),
    Wheel(WheelState),
    Joystick(JoystickState),
}

/// A flight stick / HOTAS: X/Y are the stick (−1..1, HID convention: pushed
/// forward = y −1, right = x +1), `twist` the Rz yaw axis, `throttle` the
/// slider or Z lever 0..1, `hat` the POV direction 0..7 clockwise from
/// up (0xf = centred), `buttons` bit n−1 = HID button usage n.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JoystickState {
    pub x: f32,
    pub y: f32,
    pub twist: f32,
    pub throttle: f32,
    pub hat: u8,
    pub buttons: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GamepadState {
    pub a: f32,
    pub b: f32,
    pub x: f32,
    pub y: f32,

    pub left_shoulder: f32,
    pub right_shoulder: f32,
    pub left_trigger: f32,
    pub right_trigger: f32,

    pub select: f32,
    pub start: f32,
    pub home: f32,
    pub left_thumb: f32,
    pub right_thumb: f32,

    pub dpad_up: f32,
    pub dpad_down: f32,
    pub dpad_left: f32,
    pub dpad_right: f32,

    pub left_stick: Vec2,
    pub right_stick: Vec2,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WheelState {
    pub steering: f32,
    pub throttle: f32,
    pub brake: f32,
    pub clutch: f32,
    pub steer_force: f32,
    /// Button bitmask: bit n-1 = HID button usage n (paddles, face buttons,
    /// shifter). Which bit is which paddle is per device; the app maps it.
    pub buttons: u32,
}

/// Haptic actuators exposed by a game controller. This is deliberately
/// separate from [`GameInputOutput`]: wheel FFB is a raw-HID protocol while
/// controller haptics are owned by the platform's game-controller API.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GamepadHapticCapabilities {
    /// At least one handle actuator can play haptics.
    pub handles: bool,
    /// Left and right handles can be addressed independently.
    pub separate_handles: bool,
}

/// A short controller-haptic sample. Repeated samples form continuous road
/// and suspension texture; a stronger sample is used for impacts.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GamepadHapticPulse {
    pub left: f32,
    pub right: f32,
    pub sharpness: f32,
    pub duration_s: f32,
}

/// A handle that writes OUTPUT reports to one game-input device — the way a
/// force-feedback wheel is driven. Platform-neutral so an app's FFB loop can
/// live on its own thread: the closure is `Send + Sync` and owns whatever the
/// platform needs (macOS: the IOHID device ref behind a mutex). Platforms
/// without raw HID output hand out none. A device whose force feedback the
/// OS drives itself (Windows DirectInput) carries a [`GameInputForce`]
/// instead, and its report writes fail.
#[derive(Clone)]
pub struct GameInputOutput {
    pub id: LiveId,
    pub vendor_id: u32,
    pub product_id: u32,
    send: std::sync::Arc<dyn Fn(u8, &[u8]) -> bool + Send + Sync>,
    force: Option<GameInputForce>,
}

impl GameInputOutput {
    pub fn new(
        id: LiveId,
        vendor_id: u32,
        product_id: u32,
        send: std::sync::Arc<dyn Fn(u8, &[u8]) -> bool + Send + Sync>,
    ) -> Self {
        Self { id, vendor_id, product_id, send, force: None }
    }

    /// A device the platform drives through its own force effects: no raw
    /// reports, a [`GameInputForce`] to write targets into.
    pub fn with_force(id: LiveId, vendor_id: u32, product_id: u32, force: GameInputForce) -> Self {
        Self {
            id,
            vendor_id,
            product_id,
            send: std::sync::Arc::new(|_, _| false),
            force: Some(force),
        }
    }

    /// The platform force channel, when the OS drives this device's effects.
    pub fn force(&self) -> Option<&GameInputForce> {
        self.force.as_ref()
    }

    /// Write one output report; `report_id` 0 means the device has none.
    /// False when the device is gone or the write failed.
    pub fn send_report(&self, report_id: u8, data: &[u8]) -> bool {
        (self.send)(report_id, data)
    }
}

impl std::fmt::Debug for GameInputOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GameInputOutput({:04x}:{:04x})", self.vendor_id, self.product_id)
    }
}

/// What an app asks of a platform-driven force-feedback device: a constant
/// force −1..1 of the device maximum (+ pushes the rim right, subject to the
/// device's own convention), and a centring spring and a damper 0..1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ForceTarget {
    pub constant: f32,
    pub spring: f32,
    pub damper: f32,
}

/// The app→platform force channel of one device. The app writes the latest
/// target from any thread (its own FFB loop); the platform's game-input poll
/// reads it and drives the OS effects. Atomics only: neither side ever
/// waits on the other.
///
/// A target is trusted for [`FORCE_STALE_AFTER`]: an app that stops writing
/// (paused, hung, gone) gets zero force. The platform also gives its effects
/// a finite duration ([`FORCE_EFFECT_DURATION`]) that each fresh write
/// extends, so a stalled POLL cannot hold a force either.
#[derive(Clone)]
pub struct GameInputForce {
    shared: Arc<ForceShared>,
}

struct ForceShared {
    epoch: Instant,
    constant: AtomicU32,
    spring: AtomicU32,
    damper: AtomicU32,
    /// Milliseconds since `epoch` of the last write, plus one (0 = never).
    stamp_ms: AtomicU64,
}

/// How long a written target stays in force without a new write.
pub const FORCE_STALE_AFTER: Duration = Duration::from_millis(100);
/// The platform effects' own duration; each fresh write restarts it.
pub const FORCE_EFFECT_DURATION: Duration = Duration::from_millis(250);
/// How often a held target restarts the effects (well inside their duration).
pub const FORCE_REFRESH_EVERY: Duration = Duration::from_millis(100);
/// Full scale of a platform force command (DirectInput's DI_FFNOMINALMAX).
pub const FORCE_NOMINAL_MAX: i32 = 10_000;

impl Default for GameInputForce {
    fn default() -> Self {
        Self::new()
    }
}

impl GameInputForce {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(ForceShared {
                epoch: Instant::now(),
                constant: AtomicU32::new(0),
                spring: AtomicU32::new(0),
                damper: AtomicU32::new(0),
                stamp_ms: AtomicU64::new(0),
            }),
        }
    }

    /// The app's side: the target from now on. Call it at least every
    /// [`FORCE_STALE_AFTER`] to hold a force.
    pub fn set(&self, target: ForceTarget) {
        let s = &self.shared;
        let unit = |v: f32, lo: f32| if v.is_finite() { v.clamp(lo, 1.0) } else { 0.0 };
        s.constant.store(unit(target.constant, -1.0).to_bits(), Ordering::Relaxed);
        s.spring.store(unit(target.spring, 0.0).to_bits(), Ordering::Relaxed);
        s.damper.store(unit(target.damper, 0.0).to_bits(), Ordering::Relaxed);
        s.stamp_ms.store(s.epoch.elapsed().as_millis() as u64 + 1, Ordering::Release);
    }

    /// The platform's side: the latest target and its age (`None` = never
    /// written).
    pub fn latest(&self) -> (ForceTarget, Option<Duration>) {
        let s = &self.shared;
        let stamp = s.stamp_ms.load(Ordering::Acquire);
        let f = |a: &AtomicU32| f32::from_bits(a.load(Ordering::Relaxed));
        let target = ForceTarget { constant: f(&s.constant), spring: f(&s.spring), damper: f(&s.damper) };
        let age = (stamp != 0).then(|| {
            Duration::from_millis((s.epoch.elapsed().as_millis() as u64 + 1).saturating_sub(stamp))
        });
        (target, age)
    }
}

/// One poll's worth of effect updates, in [`FORCE_NOMINAL_MAX`] units:
/// `Some` = set that parameter, `restart` = (re)start the effects so they
/// run another [`FORCE_EFFECT_DURATION`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ForceCommands {
    pub constant: Option<i32>,
    pub spring: Option<i32>,
    pub damper: Option<i32>,
    pub restart: bool,
}

/// The platform-free half of a backend's force driving: from the app's
/// target and its age to the effect updates, sending only changes, never
/// restarting on a stale target. A backend keeps one per device and applies
/// what [`ForceApplier::step`] returns.
#[derive(Clone, Debug, Default)]
pub struct ForceApplier {
    sent: Option<[i32; 3]>,
    last_restart: Option<Instant>,
}

impl ForceApplier {
    pub fn step(&mut self, target: ForceTarget, age: Option<Duration>, now: Instant) -> ForceCommands {
        let fresh = age.is_some_and(|a| a <= FORCE_STALE_AFTER);
        let scale = |v: f32| (v.clamp(-1.0, 1.0) * FORCE_NOMINAL_MAX as f32).round() as i32;
        let want = if fresh {
            [scale(target.constant), scale(target.spring.max(0.0)), scale(target.damper.max(0.0))]
        } else {
            [0; 3]
        };
        let prev = self.sent;
        let changed = |i: usize| (prev.map(|p| p[i]) != Some(want[i])).then_some(want[i]);
        let mut out = ForceCommands { constant: changed(0), spring: changed(1), damper: changed(2), restart: false };
        let active = want != [0; 3];
        if fresh && active {
            let due = self.last_restart.map_or(true, |t| now.duration_since(t) >= FORCE_REFRESH_EVERY);
            let resumed = prev.map_or(true, |p| p == [0; 3]);
            if due || resumed {
                out.restart = true;
                self.last_restart = Some(now);
            }
        } else if !active {
            self.last_restart = None;
        }
        self.sent = Some(want);
        out
    }
}

/// Conversion to and from the Studio wire form.
///
/// Both directions live here, together, on purpose: this is a 21-field
/// mapping, and a copy of it on each side of the wire is a mapping that will
/// eventually disagree with itself — one side gains a button and the other
/// silently reports zero for it. The round-trip test below is what makes that
/// impossible rather than merely unlikely.
impl From<&GameInputState> for makepad_studio_protocol::RemoteGameInput {
    fn from(state: &GameInputState) -> Self {
        use makepad_studio_protocol::{RemoteGameInput, RemoteGamepad, RemoteJoystick, RemoteWheel};
        match state {
            GameInputState::Gamepad(p) => RemoteGameInput::Gamepad(RemoteGamepad {
                a: p.a,
                b: p.b,
                x: p.x,
                y: p.y,
                left_shoulder: p.left_shoulder,
                right_shoulder: p.right_shoulder,
                left_trigger: p.left_trigger,
                right_trigger: p.right_trigger,
                select: p.select,
                start: p.start,
                home: p.home,
                left_thumb: p.left_thumb,
                right_thumb: p.right_thumb,
                dpad_up: p.dpad_up,
                dpad_down: p.dpad_down,
                dpad_left: p.dpad_left,
                dpad_right: p.dpad_right,
                left_stick_x: p.left_stick.x,
                left_stick_y: p.left_stick.y,
                right_stick_x: p.right_stick.x,
                right_stick_y: p.right_stick.y,
            }),
            GameInputState::Wheel(w) => RemoteGameInput::Wheel(RemoteWheel {
                steering: w.steering,
                throttle: w.throttle,
                brake: w.brake,
                clutch: w.clutch,
                steer_force: w.steer_force,
                buttons: w.buttons,
            }),
            GameInputState::Joystick(j) => RemoteGameInput::Joystick(RemoteJoystick {
                x: j.x,
                y: j.y,
                twist: j.twist,
                throttle: j.throttle,
                hat: j.hat,
                buttons: j.buttons,
            }),
        }
    }
}

impl From<makepad_studio_protocol::RemoteGameInput> for GameInputState {
    fn from(remote: makepad_studio_protocol::RemoteGameInput) -> Self {
        use crate::makepad_math::vec2;
        use makepad_studio_protocol::RemoteGameInput;
        match remote {
            RemoteGameInput::Gamepad(p) => GameInputState::Gamepad(GamepadState {
                a: p.a,
                b: p.b,
                x: p.x,
                y: p.y,
                left_shoulder: p.left_shoulder,
                right_shoulder: p.right_shoulder,
                left_trigger: p.left_trigger,
                right_trigger: p.right_trigger,
                select: p.select,
                start: p.start,
                home: p.home,
                left_thumb: p.left_thumb,
                right_thumb: p.right_thumb,
                dpad_up: p.dpad_up,
                dpad_down: p.dpad_down,
                dpad_left: p.dpad_left,
                dpad_right: p.dpad_right,
                left_stick: vec2(p.left_stick_x, p.left_stick_y),
                right_stick: vec2(p.right_stick_x, p.right_stick_y),
            }),
            RemoteGameInput::Wheel(w) => GameInputState::Wheel(WheelState {
                steering: w.steering,
                throttle: w.throttle,
                brake: w.brake,
                clutch: w.clutch,
                steer_force: w.steer_force,
                buttons: w.buttons,
            }),
            RemoteGameInput::Joystick(j) => GameInputState::Joystick(JoystickState {
                x: j.x,
                y: j.y,
                twist: j.twist,
                throttle: j.throttle,
                hat: j.hat,
                buttons: j.buttons,
            }),
        }
    }
}

pub struct GameInputEventChannel {
    pub sender: Sender<GameInputConnectedEvent>,
    pub receiver: Receiver<GameInputConnectedEvent>,
}

impl Default for GameInputEventChannel {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Self { sender, receiver }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_studio_protocol::RemoteGameInput;

    /// Every field gets a DIFFERENT value on purpose. A mapping that crosses
    /// two wires — sending `b` where `a` belongs — survives any test built on
    /// a uniform fixture, because both sides read the same number. Distinct
    /// values are what make a swap observable.
    fn distinctive_pad() -> GamepadState {
        GamepadState {
            a: 0.01,
            b: 0.02,
            x: 0.03,
            y: 0.04,
            left_shoulder: 0.05,
            right_shoulder: 0.06,
            left_trigger: 0.07,
            right_trigger: 0.08,
            select: 0.09,
            start: 0.10,
            home: 0.11,
            left_thumb: 0.12,
            right_thumb: 0.13,
            dpad_up: 0.14,
            dpad_down: 0.15,
            dpad_left: 0.16,
            dpad_right: 0.17,
            left_stick: crate::makepad_math::vec2(0.18, 0.19),
            right_stick: crate::makepad_math::vec2(0.20, 0.21),
        }
    }

    #[test]
    fn a_platform_force_holds_only_while_the_app_writes() {
        let force = GameInputForce::new();
        let mut applier = ForceApplier::default();
        let t0 = Instant::now();
        // Never written: nothing to do, nothing started.
        let (target, age) = force.latest();
        assert_eq!(age, None);
        let c = applier.step(target, age, t0);
        assert_eq!(c, ForceCommands { constant: Some(0), spring: Some(0), damper: Some(0), restart: false });
        // A write: the changed parameters and a start.
        force.set(ForceTarget { constant: 0.5, spring: 0.3, damper: 0.0 });
        let (target, age) = force.latest();
        assert!(age.unwrap() < FORCE_STALE_AFTER);
        let c = applier.step(target, age, t0);
        assert_eq!(c, ForceCommands { constant: Some(5000), spring: Some(3000), damper: None, restart: true });
        // The same target a frame later: no traffic until the refresh is due.
        let c = applier.step(target, age, t0 + Duration::from_millis(16));
        assert_eq!(c, ForceCommands::default());
        let c = applier.step(target, age, t0 + FORCE_REFRESH_EVERY);
        assert_eq!(c, ForceCommands { restart: true, ..Default::default() }, "a held force is restarted inside its duration");
        assert!(FORCE_REFRESH_EVERY < FORCE_EFFECT_DURATION);
        // Out-of-range and non-finite writes are clamped / zeroed.
        force.set(ForceTarget { constant: -7.0, spring: f32::NAN, damper: 2.0 });
        let (target, _) = force.latest();
        assert_eq!(target, ForceTarget { constant: -1.0, spring: 0.0, damper: 1.0 });
        // The app stops writing: zeros, and never a restart.
        let stale = Some(FORCE_STALE_AFTER + Duration::from_millis(1));
        let c = applier.step(target, stale, t0 + Duration::from_secs(1));
        assert_eq!(c, ForceCommands { constant: Some(0), spring: Some(0), damper: None, restart: false });
        let c = applier.step(target, stale, t0 + Duration::from_secs(2));
        assert_eq!(c, ForceCommands::default(), "stale stays quiet");
        // Writing again resumes at once, with a start.
        let c = applier.step(ForceTarget { constant: 0.1, ..Default::default() }, Some(Duration::ZERO), t0 + Duration::from_millis(2010));
        assert_eq!(c, ForceCommands { constant: Some(1000), spring: None, damper: None, restart: true });
    }

    #[test]
    fn a_force_output_is_not_a_report_writer() {
        let force = GameInputForce::new();
        let out = GameInputOutput::with_force(LiveId(9), 0x046d, 0xc24f, force.clone());
        assert!(!out.send_report(0, &[1, 2, 3]), "no raw reports on a platform-driven device");
        out.force().unwrap().set(ForceTarget { constant: 0.25, ..Default::default() });
        assert_eq!(force.latest().0.constant, 0.25, "the handle and the platform share one channel");
        let raw = GameInputOutput::new(LiveId(1), 0, 0, Arc::new(|_, _| true));
        assert!(raw.force().is_none());
    }

    #[test]
    fn a_gamepad_survives_the_trip_to_studio_and_back() {
        let original = GameInputState::Gamepad(distinctive_pad());
        let wire: RemoteGameInput = (&original).into();
        let returned: GameInputState = wire.into();
        assert_eq!(original, returned);
    }

    #[test]
    fn a_wheel_survives_the_trip_to_studio_and_back() {
        let original = GameInputState::Wheel(WheelState {
            steering: 0.31,
            throttle: 0.32,
            brake: 0.33,
            clutch: 0.34,
            steer_force: 0.35,
            buttons: 0b1011,
        });
        let wire: RemoteGameInput = (&original).into();
        let returned: GameInputState = wire.into();
        assert_eq!(original, returned);
    }

    #[test]
    fn a_joystick_survives_the_trip_to_studio_and_back() {
        let original = GameInputState::Joystick(JoystickState {
            x: -0.4,
            y: 0.9,
            twist: 0.2,
            throttle: 0.75,
            hat: 6,
            buttons: 0b10_0101,
        });
        let wire: RemoteGameInput = (&original).into();
        let returned: GameInputState = wire.into();
        assert_eq!(original, returned);
    }

    /// The sticks are the only fields that change shape across the wire (a
    /// Vec2 flattened to two scalars), so they are the likeliest to get
    /// crossed. Pinned separately rather than trusting the round trip, which
    /// would still pass if x and y were swapped in BOTH directions.
    #[test]
    fn stick_axes_do_not_cross_on_the_wire() {
        let wire: RemoteGameInput = (&GameInputState::Gamepad(distinctive_pad())).into();
        let RemoteGameInput::Gamepad(pad) = wire else {
            panic!("a gamepad must not arrive as a wheel");
        };
        assert_eq!(pad.left_stick_x, 0.18);
        assert_eq!(pad.left_stick_y, 0.19);
        assert_eq!(pad.right_stick_x, 0.20);
        assert_eq!(pad.right_stick_y, 0.21);
    }
}
