use crate::makepad_live_id::*;
use crate::makepad_math::Vec2;
use std::sync::mpsc::{channel, Receiver, Sender};

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

/// A handle that writes OUTPUT reports to one game-input device — the way a
/// force-feedback wheel is driven. Platform-neutral so an app's FFB loop can
/// live on its own thread: the closure is `Send + Sync` and owns whatever the
/// platform needs (macOS: the IOHID device ref behind a mutex). Platforms
/// without raw HID output hand out none.
#[derive(Clone)]
pub struct GameInputOutput {
    pub id: LiveId,
    pub vendor_id: u32,
    pub product_id: u32,
    send: std::sync::Arc<dyn Fn(u8, &[u8]) -> bool + Send + Sync>,
}

impl GameInputOutput {
    pub fn new(
        id: LiveId,
        vendor_id: u32,
        product_id: u32,
        send: std::sync::Arc<dyn Fn(u8, &[u8]) -> bool + Send + Sync>,
    ) -> Self {
        Self { id, vendor_id, product_id, send }
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
