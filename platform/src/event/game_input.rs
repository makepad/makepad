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
