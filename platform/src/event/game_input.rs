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
    // Add other common wheel inputs like gear shifter buttons if needed, keeping it simple for now
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
        use makepad_studio_protocol::{RemoteGameInput, RemoteGamepad, RemoteWheel};
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
